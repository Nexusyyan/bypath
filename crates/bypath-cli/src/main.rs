use anyhow::{bail, Result};
use bypath_core::capture::CaptureEngine;
use bypath_core::config::{
    builtin_preset_rule, default_config_path, Config, GeneralConfig, ProxyConfig, RuleAction,
    RuleConfig,
};
use bypath_core::engine::ProxyEngine;
use bypath_core::process::{ProcessTracker, SnapshotProcessTracker};
use bypath_core::upstream::probe::probe_proxy;
use bypath_daemon::{
    install_service, query_service_status, start_service, stop_service, uninstall_service,
    SERVICE_NAME,
};
use bypath_ipc::{IpcClient, IpcRequest, IpcResponse, DEFAULT_PIPE_NAME};
use bypath_windivert::{is_elevated, probe_windivert, WinDivertCaptureEngine};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "bypath",
    version,
    about = "Open-source per-application transparent proxy bridge (SOCKS5 / HTTP)"
)]
struct Cli {
    /// Override log level (trace, debug, info, warn, error)
    #[arg(long, global = true)]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run environment, WinDivert driver, and upstream proxy diagnostics
    Doctor {
        /// Optional upstream proxy URI to probe (e.g. socks5://127.0.0.1:7890)
        #[arg(long)]
        proxy: Option<String>,

        /// Optional config file whose proxies will all be probed
        #[arg(long, short)]
        config: Option<PathBuf>,

        /// Directory containing WinDivert.dll and WinDivert64.sys
        #[arg(long, default_value = "runtime/windivert")]
        windivert_dir: PathBuf,
    },

    /// List running processes (PID, PPID, name, executable path) to help write rules
    Ps {
        /// Case-insensitive substring filter applied to process name and path
        #[arg(long, short)]
        filter: Option<String>,
    },

    /// Start transparent per-application traffic proxying in the foreground
    Run {
        /// Path to TOML configuration file
        #[arg(long, short)]
        config: Option<PathBuf>,

        /// Quick mode: upstream proxy URI (socks5://127.0.0.1:7890 or http://127.0.0.1:7890)
        #[arg(long)]
        proxy: Option<String>,

        /// Quick mode: enable one or more built-in presets (antigravity, cursor, vscode, git, discord, steam, telegram)
        #[arg(long = "preset")]
        presets: Vec<String>,

        /// Quick mode: target executable name(s) (e.g. --app curl.exe)
        #[arg(long = "app")]
        apps: Vec<String>,

        /// Quick mode: match any process whose executable path contains this substring
        #[arg(long = "path-contains")]
        path_contains: Vec<String>,

        /// Quick mode: block specific domain(s) or wildcard patterns (e.g. --block-domain *.doubleclick.net)
        #[arg(long = "block-domain")]
        block_domains: Vec<String>,

        /// Disable automatic child-process-tree inheritance in quick mode
        #[arg(long)]
        no_tree: bool,

        /// Disable SOCKS5 UDP ASSOCIATE in quick mode (triggers QUIC TCP fallback)
        #[arg(long)]
        no_udp: bool,

        /// Disable TLS SNI / HTTP Host protocol sniffing in quick mode
        #[arg(long)]
        no_sniff: bool,

        /// Directory containing WinDivert.dll and WinDivert64.sys
        #[arg(long)]
        windivert_dir: Option<PathBuf>,
    },

    /// Manage the Windows background service (BypathService)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },

    /// Control a running Bypath daemon over Named Pipe IPC
    Ctl {
        #[arg(long, default_value = DEFAULT_PIPE_NAME)]
        pipe: String,

        #[command(subcommand)]
        action: CtlAction,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceAction {
    /// Install BypathService pointing to sibling bypath-daemon.exe
    Install {
        #[arg(long, short)]
        config: Option<PathBuf>,

        #[arg(long)]
        daemon_exe: Option<PathBuf>,

        #[arg(long)]
        demand_start: bool,
    },
    /// Stop and remove BypathService
    Uninstall,
    /// Start BypathService
    Start,
    /// Stop BypathService
    Stop,
    /// Print BypathService installation and running status
    Status,
}

#[derive(Subcommand, Debug)]
enum CtlAction {
    /// Show live daemon status and traffic stats
    Status,
    /// Start packet interception in the daemon
    Start,
    /// Stop packet interception in the daemon
    Stop,
    /// Reload a TOML config file into the running daemon without dropping connections
    Reload {
        #[arg(long, short)]
        config: PathBuf,
    },
    /// Display live and recent connections tracked by the daemon
    Connections,
    /// Clear closed connections from the daemon history
    Clear,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let default_level = cli.log_level.as_deref().unwrap_or("info");
    init_tracing(default_level);

    match cli.command {
        Commands::Doctor {
            proxy,
            config,
            windivert_dir,
        } => run_doctor(proxy, config, &windivert_dir).await,
        Commands::Ps { filter } => run_ps(filter.as_deref()),
        Commands::Run {
            config,
            proxy,
            presets,
            apps,
            path_contains,
            block_domains,
            no_tree,
            no_udp,
            no_sniff,
            windivert_dir,
        } => {
            let cfg = build_run_config(
                config.as_deref(),
                proxy.as_deref(),
                &presets,
                &apps,
                &path_contains,
                &block_domains,
                !no_tree,
                !no_udp,
                !no_sniff,
                windivert_dir.as_deref(),
            )?;
            run_engine(cfg).await
        }
        Commands::Service { action } => run_service_cmd(action),
        Commands::Ctl { pipe, action } => run_ctl_cmd(&pipe, action).await,
    }
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "bypath_core={level},bypath_windivert={level},bypath_ipc={level},bypath_daemon={level},bypath={level},{level}"
        ))
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}

fn run_service_cmd(action: ServiceAction) -> Result<()> {
    match action {
        ServiceAction::Install {
            config,
            daemon_exe,
            demand_start,
        } => {
            let exe = match daemon_exe {
                Some(p) => p,
                None => {
                    let sibling = std::env::current_exe()?
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join("bypath-daemon.exe");
                    if !sibling.exists() {
                        bail!(
                            "could not find '{}' next to bypath.exe; build bypath-daemon or pass --daemon-exe",
                            sibling.display()
                        );
                    }
                    sibling
                }
            };
            let cfg_path = config.unwrap_or_else(default_config_path);
            install_service(&exe, &cfg_path, !demand_start)?;
            println!(
                "Installed Windows Service '{SERVICE_NAME}' ({}) with config {}",
                exe.display(),
                cfg_path.display()
            );
            Ok(())
        }
        ServiceAction::Uninstall => {
            uninstall_service()?;
            println!("Uninstalled Windows Service '{SERVICE_NAME}'.");
            Ok(())
        }
        ServiceAction::Start => {
            start_service()?;
            println!("Started Windows Service '{SERVICE_NAME}'.");
            Ok(())
        }
        ServiceAction::Stop => {
            stop_service()?;
            println!("Stopped Windows Service '{SERVICE_NAME}'.");
            Ok(())
        }
        ServiceAction::Status => {
            let s = query_service_status();
            println!(
                "Service '{SERVICE_NAME}': installed={} running={} state={}",
                s.installed, s.running, s.state
            );
            Ok(())
        }
    }
}

async fn run_ctl_cmd(pipe: &str, action: CtlAction) -> Result<()> {
    let client = IpcClient::new(pipe);
    match action {
        CtlAction::Status => match client.request(&IpcRequest::GetStatus).await? {
            IpcResponse::Status { status } => {
                println!("=== Bypath Daemon Status ===");
                println!("Mode             : {:?} (PID {})", status.backend_mode, status.daemon_pid);
                println!("Engine           : {}", if status.engine_running { "RUNNING" } else { "STOPPED" });
                println!("Elevated         : {}", status.elevated);
                println!(
                    "WinDivert        : {}",
                    if status.windivert_ready {
                        format!("OK ({})", status.windivert_version.as_deref().unwrap_or("loaded"))
                    } else {
                        status.windivert_error.unwrap_or_else(|| "NOT READY".into())
                    }
                );
                println!("Config           : {}", status.config_path);
                println!(
                    "Rules / Proxies  : {}/{} enabled rules, {} proxies",
                    status.active_rules_count, status.total_rules_count, status.proxies_count
                );
                println!(
                    "Traffic          : active={} total={} tx={} B ({} B/s) rx={} B ({} B/s)",
                    status.stats.active_connections,
                    status.stats.total_connections,
                    status.stats.total_bytes_tx,
                    status.stats.tx_bytes_per_sec,
                    status.stats.total_bytes_rx,
                    status.stats.rx_bytes_per_sec
                );
                Ok(())
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected response: {other:?}"),
        },
        CtlAction::Start => match client.request(&IpcRequest::StartEngine).await? {
            IpcResponse::Ok { message } => {
                println!("{message}");
                Ok(())
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected response: {other:?}"),
        },
        CtlAction::Stop => match client.request(&IpcRequest::StopEngine).await? {
            IpcResponse::Ok { message } => {
                println!("{message}");
                Ok(())
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected response: {other:?}"),
        },
        CtlAction::Reload { config } => {
            let cfg = Config::from_file(&config)?;
            match client
                .request(&IpcRequest::SaveConfig {
                    config: cfg,
                    reload: true,
                })
                .await?
            {
                IpcResponse::Ok { message } => {
                    println!("Reloaded {}: {message}", config.display());
                    Ok(())
                }
                IpcResponse::Error { message } => bail!("{message}"),
                other => bail!("unexpected response: {other:?}"),
            }
        }
        CtlAction::Connections => match client.request(&IpcRequest::GetConnections).await? {
            IpcResponse::Connections { connections } => {
                println!(
                    "{:<6} {:<22} {:<6} {:<36} {:<10} {:<10} {:<10}",
                    "ID", "PROCESS", "PROTO", "TARGET", "STATE", "TX", "RX"
                );
                println!("{}", "-".repeat(106));
                for c in connections {
                    let target = match c.sniffed_host {
                        Some(ref h) => format!("{h}:{}", c.dst_port),
                        None => format!("{}:{}", c.dst_ip, c.dst_port),
                    };
                    println!(
                        "{:<6} {:<22} {:<6} {:<36} {:<10} {:<10} {:<10}",
                        c.id,
                        c.process_name,
                        format!("{:?}", c.protocol).to_uppercase(),
                        target,
                        format!("{:?}", c.state),
                        c.bytes_tx,
                        c.bytes_rx
                    );
                }
                Ok(())
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected response: {other:?}"),
        },
        CtlAction::Clear => match client.request(&IpcRequest::ClearConnections).await? {
            IpcResponse::Ok { message } => {
                println!("{message}");
                Ok(())
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected response: {other:?}"),
        },
    }
}

fn run_ps(filter: Option<&str>) -> Result<()> {
    let tracker = SnapshotProcessTracker::default();
    let needle = filter.map(|s| s.to_ascii_lowercase());
    let procs = tracker.list_processes();

    println!("{:<8} {:<8} {:<32} PATH", "PID", "PPID", "NAME");
    println!("{}", "-".repeat(96));

    let mut shown = 0usize;
    for p in procs {
        let path_str = p.exe_path.as_deref().unwrap_or("-");
        if let Some(ref n) = needle {
            let matches_name = p.name.to_ascii_lowercase().contains(n);
            let matches_path = path_str.to_ascii_lowercase().contains(n);
            if !matches_name && !matches_path {
                continue;
            }
        }
        println!("{:<8} {:<8} {:<32} {}", p.pid, p.parent_pid, p.name, path_str);
        shown += 1;
    }

    println!("\nTotal matched processes: {shown}");
    Ok(())
}

async fn run_doctor(
    proxy_uri: Option<String>,
    config_path: Option<PathBuf>,
    windivert_dir: &Path,
) -> Result<()> {
    println!("=== Bypath Doctor ===\n");

    let elevated = is_elevated();
    println!(
        "[1/3] Administrator privileges : {}",
        if elevated {
            "OK (elevated)"
        } else {
            "WARN (not elevated — live capture requires Administrator PowerShell or Windows Service)"
        }
    );

    let wd = probe_windivert(Some(windivert_dir));
    if wd.dll_loaded && wd.driver_opened {
        let ver = wd
            .driver_version
            .map(|(maj, min)| format!("v{maj}.{min}"))
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "[2/3] WinDivert kernel driver  : OK ({}, driver {ver})",
            wd.dll_path.as_deref().unwrap_or("WinDivert.dll")
        );
    } else if wd.dll_loaded {
        println!(
            "[2/3] WinDivert kernel driver  : FAIL (DLL loaded at {}, but driver open failed)",
            wd.dll_path.as_deref().unwrap_or("WinDivert.dll")
        );
        if let Some(ref err) = wd.error {
            println!("      Detail: {err}");
        }
    } else {
        println!("[2/3] WinDivert kernel driver  : MISSING");
        if let Some(ref err) = wd.error {
            println!("      Detail: {err}");
        }
        println!("      Hint  : Run `.\\scripts\\fetch-windivert.ps1` (and allow WinDivert in Windows Security if quarantined).");
    }

    let mut proxies_to_probe: Vec<(String, ProxyConfig)> = Vec::new();
    if let Some(uri) = proxy_uri {
        proxies_to_probe.push(("cli".to_string(), ProxyConfig::from_uri(&uri, true)?));
    }
    if let Some(cfg_path) = config_path {
        let cfg = Config::from_file(&cfg_path)?;
        for (name, p) in cfg.proxies {
            proxies_to_probe.push((name, p));
        }
    }

    if proxies_to_probe.is_empty() {
        println!("[3/3] Upstream proxy check     : SKIPPED (pass --proxy socks5://127.0.0.1:10808 or --config <file>)");
    } else {
        println!("[3/3] Upstream proxy check     :");
        for (name, pcfg) in proxies_to_probe {
            let report = probe_proxy(&pcfg, Duration::from_secs(3)).await;
            if report.tcp_reachable {
                let udp_str = match report.udp_associate_supported {
                    Some(true) => "UDP ASSOCIATE OK",
                    Some(false) => "UDP unavailable (QUIC fallback blocking recommended)",
                    None => "UDP unknown",
                };
                let pid_note = pcfg
                    .parse_socket_addr()
                    .ok()
                    .filter(|sa| sa.ip().is_loopback())
                    .and_then(|sa| bypath_windivert::iphelper::find_tcp_listener_pid(sa.port()))
                    .map(|pid| format!(", local listener PID={pid} [auto anti-loop]"))
                    .unwrap_or_default();

                println!(
                    "      - {name} ({:?} @ {}): OK ({} ms, {udp_str}{pid_note})",
                    report.kind,
                    report.address,
                    report.latency_ms.unwrap_or(0)
                );
            } else {
                println!(
                    "      - {name} ({:?} @ {}): FAIL ({})",
                    report.kind,
                    report.address,
                    report.error.as_deref().unwrap_or("unreachable")
                );
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_run_config(
    config_path: Option<&Path>,
    proxy_uri: Option<&str>,
    presets: &[String],
    apps: &[String],
    path_contains: &[String],
    block_domains: &[String],
    include_children: bool,
    enable_udp: bool,
    enable_sniffing: bool,
    windivert_dir: Option<&Path>,
) -> Result<Config> {
    let mut cfg = if let Some(path) = config_path {
        Config::from_file(path)?
    } else {
        let uri = proxy_uri.ok_or_else(|| {
            anyhow::anyhow!(
                "provide either --config <file> or --proxy <socks5://host:port> with --preset/--app"
            )
        })?;
        let proxy_cfg = ProxyConfig::from_uri(uri, enable_udp)?;
        let mut proxies = HashMap::new();
        proxies.insert("default".to_string(), proxy_cfg);

        let mut rules = Vec::new();
        if !block_domains.is_empty() {
            rules.push(RuleConfig {
                name: "CLI Domain Block".to_string(),
                enabled: true,
                process_names: vec![],
                path_prefixes: vec![],
                path_contains: vec![],
                include_children: true,
                dst_cidrs: vec![],
                dst_ports: vec![],
                protocols: vec![],
                domains: block_domains.to_vec(),
                action: RuleAction::Block,
                proxy: None,
            });
        }
        for preset in presets {
            rules.push(builtin_preset_rule(preset, "default")?);
        }
        if !apps.is_empty() || !path_contains.is_empty() {
            rules.push(RuleConfig {
                name: "CLI Quick Rule".to_string(),
                enabled: true,
                process_names: apps.to_vec(),
                path_prefixes: vec![],
                path_contains: path_contains.to_vec(),
                include_children,
                dst_cidrs: vec![],
                dst_ports: vec![],
                protocols: vec![],
                domains: vec![],
                action: RuleAction::Proxy,
                proxy: Some("default".to_string()),
            });
        }

        if rules.is_empty() {
            bail!(
                "no target apps specified: pass --preset antigravity, --app <exe>, or --path-contains <substr>"
            );
        }

        Config {
            general: GeneralConfig {
                enable_sniffing,
                ..GeneralConfig::default()
            },
            proxies,
            rules,
        }
    };

    if let Some(dir) = windivert_dir {
        cfg.general.windivert_dir = dir.display().to_string();
    }
    cfg.validate()?;
    Ok(cfg)
}

async fn run_engine(cfg: Config) -> Result<()> {
    if !is_elevated() {
        warn!("process does not appear to be elevated; WinDivert driver open will likely fail unless running as Administrator");
    }

    let wd_dir = PathBuf::from(&cfg.general.windivert_dir);
    let tracker: Arc<dyn ProcessTracker> = Arc::new(SnapshotProcessTracker::default());
    let capture: Arc<dyn CaptureEngine> = Arc::new(WinDivertCaptureEngine::with_process_tracker(
        Some(&wd_dir),
        Some(Arc::clone(&tracker)),
    )?);

    info!(
        rules = cfg.rules.len(),
        proxies = cfg.proxies.len(),
        sniffing = cfg.general.enable_sniffing,
        bypass_lan = cfg.general.bypass_lan,
        "initializing bypath engine"
    );

    let engine = ProxyEngine::new(cfg, capture, tracker)?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(());
        }
    });

    engine.run(shutdown_rx).await
}
