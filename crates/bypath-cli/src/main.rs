mod agent;
mod mcp;

use crate::agent::{capability_schema, parse_proxy_kind, parse_rule_action, AgentClient};
use anyhow::{bail, Result};
use bypath_core::capture::CaptureEngine;
use bypath_core::config::{
    builtin_preset_rule, default_config_path, Config, GeneralConfig, ProxyConfig, ProxyKind,
    RuleAction, RuleConfig,
};
use bypath_core::engine::ProxyEngine;
use bypath_core::process::{ProcessTracker, SnapshotProcessTracker};
use bypath_core::rules::RuleTestQuery;
use bypath_core::upstream::probe::probe_proxy;
use bypath_daemon::{
    install_service, query_service_status, start_service, stop_service, uninstall_service,
    SERVICE_NAME,
};
use bypath_ipc::DEFAULT_PIPE_NAME;
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

    /// Control Bypath over Named Pipe IPC (with offline fallback and --json for AI Agents)
    Ctl {
        #[arg(long, default_value = DEFAULT_PIPE_NAME)]
        pipe: String,

        /// Optional config path override when running offline
        #[arg(long)]
        config: Option<PathBuf>,

        /// Emit structured machine-readable JSON output
        #[arg(long, global = true)]
        json: bool,

        #[command(subcommand)]
        action: CtlAction,
    },

    /// Run a Stdio JSON-RPC 2.0 MCP server for AI Agent DIY automation
    Mcp {
        #[arg(long, default_value = DEFAULT_PIPE_NAME)]
        pipe: String,

        /// Optional config path override when running offline
        #[arg(long)]
        config: Option<PathBuf>,
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
    /// Run full daemon diagnostics (driver + proxies)
    Doctor,
    /// Read the Windows system proxy (and optionally apply it to the default proxy profile)
    SystemProxy {
        /// Apply the discovered system proxy to the first/default proxy profile
        #[arg(long)]
        apply: bool,

        /// Target proxy profile name when --apply is set (defaults to local_socks)
        #[arg(long, default_value = "local_socks")]
        name: String,
    },
    /// Reload a TOML config file into the running daemon without dropping connections
    Reload {
        #[arg(long, short)]
        config: PathBuf,
    },
    /// Read, export, or import configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Manage upstream proxy profiles (list, set, remove, probe)
    Proxy {
        #[command(subcommand)]
        action: ProxyAction,
    },
    /// Manage routing rules (list, add, update, move, toggle, remove, test)
    Rule {
        #[command(subcommand)]
        action: RuleActionCmd,
    },
    /// Display live and recent connections tracked by the daemon
    Connections {
        #[arg(long)]
        process: Option<String>,
        #[arg(long)]
        state: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Clear closed connections from the daemon history
    Clear,
    /// Retrieve recent structured diagnostic logs from the daemon
    Logs {
        #[arg(long)]
        level: Option<String>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },
    /// Check GitHub Releases for a newer Bypath version (proxy-aware)
    Update {
        /// Optional explicit proxy URI override (e.g. socks5://127.0.0.1:10808)
        #[arg(long)]
        proxy: Option<String>,
    },
    /// Print machine-readable capability manifest and schema for AI Agents
    Schema,
}

#[derive(Subcommand, Debug)]
enum ConfigAction {
    /// Print active configuration
    Get,
    /// Export current configuration to a TOML file
    Export { path: PathBuf },
    /// Import and hot-reload configuration from a TOML file
    Import { path: PathBuf },
}

#[derive(Subcommand, Debug)]
enum ProxyAction {
    /// List configured upstream proxies
    List,
    /// Create or update an upstream proxy profile
    Set {
        name: String,
        #[arg(long = "type", default_value = "socks5")]
        kind: String,
        #[arg(long)]
        addr: String,
        #[arg(long)]
        user: Option<String>,
        #[arg(long)]
        pass: Option<String>,
        #[arg(long, default_value_t = true)]
        udp: bool,
        #[arg(long)]
        rename: Option<String>,
    },
    /// Remove an unreferenced proxy profile
    Remove { name: String },
    /// Probe reachability, handshake, and UDP support for a configured proxy
    Probe { name: String },
}

#[derive(Subcommand, Debug)]
enum RuleActionCmd {
    /// List rules in priority order
    List,
    /// Add a new routing rule
    Add {
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "proxy")]
        action: String,
        #[arg(long)]
        proxy: Option<String>,
        #[arg(long = "process")]
        process_names: Vec<String>,
        #[arg(long = "path-prefix")]
        path_prefixes: Vec<String>,
        #[arg(long = "path-contains")]
        path_contains: Vec<String>,
        #[arg(long = "domain")]
        domains: Vec<String>,
        #[arg(long = "cidr")]
        dst_cidrs: Vec<String>,
        #[arg(long = "port")]
        dst_ports: Vec<u16>,
        #[arg(long = "proto")]
        protocols: Vec<String>,
        #[arg(long)]
        no_tree: bool,
        #[arg(long)]
        prepend: bool,
    },
    /// Update an existing rule by 0-based index or exact name
    Update {
        selector: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        action: Option<String>,
        #[arg(long)]
        proxy: Option<String>,
        #[arg(long = "process")]
        process_names: Option<Vec<String>>,
        #[arg(long = "path-prefix")]
        path_prefixes: Option<Vec<String>>,
        #[arg(long = "path-contains")]
        path_contains: Option<Vec<String>>,
        #[arg(long = "domain")]
        domains: Option<Vec<String>>,
        #[arg(long = "cidr")]
        dst_cidrs: Option<Vec<String>>,
        #[arg(long = "port")]
        dst_ports: Option<Vec<u16>>,
        #[arg(long = "proto")]
        protocols: Option<Vec<String>>,
        #[arg(long)]
        include_children: Option<bool>,
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Reorder rule priority (move from_index -> to_index)
    Move { from: usize, to: usize },
    /// Enable or disable a rule by 0-based index or name
    Toggle {
        selector: String,
        #[arg(long)]
        enabled: Option<bool>,
    },
    /// Delete a rule by 0-based index or name
    Remove { selector: String },
    /// Dry-run rule matching for a hypothetical flow
    Test {
        #[arg(long)]
        process: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        ip: Option<String>,
        #[arg(long, default_value_t = 443)]
        port: u16,
        #[arg(long, default_value = "tcp")]
        proto: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let default_level = cli.log_level.as_deref().unwrap_or("info");

    match cli.command {
        Commands::Mcp { pipe, config } => {
            // Keep stdout strictly clean for JSON-RPC 2.0 frames.
            init_tracing(default_level, true);
            mcp::run_mcp_stdio(pipe, config).await
        }
        Commands::Doctor {
            proxy,
            config,
            windivert_dir,
        } => {
            init_tracing(default_level, false);
            run_doctor(proxy, config, &windivert_dir).await
        }
        Commands::Ps { filter } => {
            init_tracing(default_level, false);
            run_ps(filter.as_deref())
        }
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
            init_tracing(default_level, false);
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
        Commands::Service { action } => {
            init_tracing(default_level, false);
            run_service_cmd(action)
        }
        Commands::Ctl {
            pipe,
            config,
            json,
            action,
        } => {
            init_tracing(default_level, json);
            run_ctl_cmd(&pipe, config, json, action).await
        }
    }
}

fn init_tracing(level: &str, to_stderr: bool) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "bypath_core={level},bypath_windivert={level},bypath_ipc={level},bypath_daemon={level},bypath={level},{level}"
        ))
    });
    if to_stderr {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .with_target(false)
            .compact()
            .try_init();
    } else {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .compact()
            .try_init();
    }
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

async fn run_ctl_cmd(
    pipe: &str,
    config_override: Option<PathBuf>,
    as_json: bool,
    action: CtlAction,
) -> Result<()> {
    if matches!(action, CtlAction::Schema) {
        println!("{}", serde_json::to_string_pretty(&capability_schema())?);
        return Ok(());
    }

    let client = AgentClient::new(pipe, config_override)?;

    match action {
        CtlAction::Status => {
            let status = client.get_status().await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("=== Bypath Daemon Status ===");
                println!(
                    "Mode             : {:?} (PID {})",
                    status.backend_mode, status.daemon_pid
                );
                println!(
                    "Engine           : {}",
                    if status.engine_running { "RUNNING" } else { "STOPPED" }
                );
                println!("Elevated         : {}", status.elevated);
                println!("Portable         : {}", status.portable_mode);
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
            }
            Ok(())
        }
        CtlAction::Start => {
            let message = client.start_engine().await?;
            if as_json {
                println!("{}", serde_json::json!({ "ok": true, "message": message }));
            } else {
                println!("{message}");
            }
            Ok(())
        }
        CtlAction::Stop => {
            let message = client.stop_engine().await?;
            if as_json {
                println!("{}", serde_json::json!({ "ok": true, "message": message }));
            } else {
                println!("{message}");
            }
            Ok(())
        }
        CtlAction::Doctor => {
            let report = client.run_doctor().await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "Elevated={} WinDivertLoaded={} DriverOpened={} Version={}",
                    report.elevated,
                    report.windivert_dll_loaded,
                    report.windivert_driver_opened,
                    report.windivert_version.as_deref().unwrap_or("-")
                );
                for p in report.proxies {
                    println!(
                        "Proxy {}: reachable={} handshake_ok={:?} latency_ms={:?} udp={:?} err={:?}",
                        p.name,
                        p.report.tcp_reachable,
                        p.report.handshake_ok,
                        p.report.latency_ms,
                        p.report.udp_associate_supported,
                        p.report.error
                    );
                }
            }
            Ok(())
        }
        CtlAction::SystemProxy { apply, name } => {
            let info = client.get_system_proxy().await?;
            if apply {
                let addr = info
                    .suggested_address
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("no active Windows system proxy found"))?;
                let kind = info.suggested_kind.unwrap_or(ProxyKind::Http);
                client
                    .upsert_proxy(&name, kind, addr, None, None, kind == ProxyKind::Socks5, None)
                    .await?;
            }
            if as_json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "applied": apply,
                        "profile": if apply { Some(name) } else { None },
                        "system_proxy": info
                    }))?
                );
            } else if info.enabled {
                println!(
                    "System proxy ({}): suggested={:?} @ {}",
                    info.source,
                    info.suggested_kind,
                    info.suggested_address.as_deref().unwrap_or("-")
                );
                if apply {
                    println!("Applied to profile '{name}'.");
                }
            } else {
                println!("No active system proxy detected.");
            }
            Ok(())
        }
        CtlAction::Reload { config } => {
            let cfg = Config::from_file(&config)?;
            let message = client.save_config(cfg, true).await?;
            if as_json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": true, "path": config, "message": message })
                );
            } else {
                println!("Reloaded {}: {message}", config.display());
            }
            Ok(())
        }
        CtlAction::Config { action } => match action {
            ConfigAction::Get => {
                let snap = client.get_config().await?;
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&snap)?);
                } else {
                    println!("# Path: {}", snap.path);
                    println!("{}", toml::to_string_pretty(&snap.config)?);
                }
                Ok(())
            }
            ConfigAction::Export { path } => {
                let snap = client.get_config().await?;
                snap.config.save_to_file(&path)?;
                if as_json {
                    println!("{}", serde_json::json!({ "ok": true, "exported_to": path }));
                } else {
                    println!("Exported configuration to {}", path.display());
                }
                Ok(())
            }
            ConfigAction::Import { path } => {
                let cfg = Config::from_file(&path)?;
                let message = client.save_config(cfg, true).await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::json!({ "ok": true, "imported_from": path, "message": message })
                    );
                } else {
                    println!("Imported {}: {message}", path.display());
                }
                Ok(())
            }
        },
        CtlAction::Proxy { action } => match action {
            ProxyAction::List => {
                let snap = client.get_config().await?;
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&snap.config.proxies)?);
                } else {
                    for (name, p) in &snap.config.proxies {
                        println!("{:<18} {:<8} {:<24} udp={}", name, format!("{:?}", p.kind), p.address, p.udp);
                    }
                }
                Ok(())
            }
            ProxyAction::Set {
                name,
                kind,
                addr,
                user,
                pass,
                udp,
                rename,
            } => {
                let k = parse_proxy_kind(&kind)?;
                let snap = client
                    .upsert_proxy(&name, k, addr, user, pass, udp, rename)
                    .await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "proxies": snap.config.proxies
                        }))?
                    );
                } else {
                    println!("Saved proxy profile '{name}'.");
                }
                Ok(())
            }
            ProxyAction::Remove { name } => {
                let snap = client.remove_proxy(&name).await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "removed": name,
                            "remaining": snap.config.proxies.len()
                        }))?
                    );
                } else {
                    println!("Removed proxy profile '{name}'.");
                }
                Ok(())
            }
            ProxyAction::Probe { name } => {
                let snap = client.get_config().await?;
                let p = snap
                    .config
                    .proxies
                    .get(&name)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("proxy '{name}' not found"))?;
                let probe = client.probe_proxy(p).await?;
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&probe)?);
                } else {
                    println!(
                        "{}: tcp_reachable={} handshake_ok={:?} latency_ms={:?} udp={:?} err={:?}",
                        name,
                        probe.report.tcp_reachable,
                        probe.report.handshake_ok,
                        probe.report.latency_ms,
                        probe.report.udp_associate_supported,
                        probe.report.error
                    );
                }
                Ok(())
            }
        },
        CtlAction::Rule { action } => match action {
            RuleActionCmd::List => {
                let snap = client.get_config().await?;
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&snap.config.rules)?);
                } else {
                    for (i, r) in snap.config.rules.iter().enumerate() {
                        println!(
                            "#{:<3} [{}] {:<28} {:<7} proxy={:<12} procs={:?} domains={:?}",
                            i,
                            if r.enabled { "ON " } else { "OFF" },
                            r.name,
                            format!("{:?}", r.action),
                            r.proxy.as_deref().unwrap_or("-"),
                            r.process_names,
                            r.domains
                        );
                    }
                }
                Ok(())
            }
            RuleActionCmd::Add {
                name,
                action,
                proxy,
                process_names,
                path_prefixes,
                path_contains,
                domains,
                dst_cidrs,
                dst_ports,
                protocols,
                no_tree,
                prepend,
            } => {
                let act = parse_rule_action(&action)?;
                let resolved_proxy = if act == RuleAction::Proxy {
                    match proxy {
                        Some(p) => Some(p),
                        None => client.get_config().await?.config.proxies.keys().next().cloned(),
                    }
                } else {
                    None
                };
                let rule = RuleConfig {
                    name,
                    enabled: true,
                    process_names,
                    path_prefixes,
                    path_contains,
                    include_children: !no_tree,
                    dst_cidrs,
                    dst_ports,
                    protocols,
                    domains,
                    action: act,
                    proxy: resolved_proxy,
                };
                let (idx, snap) = client.upsert_rule(None, rule, prepend).await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "index": idx,
                            "rule": snap.config.rules[idx]
                        }))?
                    );
                } else {
                    println!("Saved rule #{idx}: {}", snap.config.rules[idx].name);
                }
                Ok(())
            }
            RuleActionCmd::Update {
                selector,
                name,
                action,
                proxy,
                process_names,
                path_prefixes,
                path_contains,
                domains,
                dst_cidrs,
                dst_ports,
                protocols,
                include_children,
                enabled,
            } => {
                let snap = client.get_config().await?;
                let idx = crate::agent::resolve_rule_index(&snap.config.rules, &selector)?;
                let mut rule = snap.config.rules[idx].clone();
                if let Some(n) = name {
                    rule.name = n;
                }
                if let Some(a) = action {
                    rule.action = parse_rule_action(&a)?;
                }
                if let Some(p) = proxy {
                    rule.proxy = if p.is_empty() { None } else { Some(p) };
                }
                if rule.action != RuleAction::Proxy {
                    rule.proxy = None;
                }
                if let Some(v) = process_names {
                    rule.process_names = v;
                }
                if let Some(v) = path_prefixes {
                    rule.path_prefixes = v;
                }
                if let Some(v) = path_contains {
                    rule.path_contains = v;
                }
                if let Some(v) = domains {
                    rule.domains = v;
                }
                if let Some(v) = dst_cidrs {
                    rule.dst_cidrs = v;
                }
                if let Some(v) = dst_ports {
                    rule.dst_ports = v;
                }
                if let Some(v) = protocols {
                    rule.protocols = v;
                }
                if let Some(v) = include_children {
                    rule.include_children = v;
                }
                if let Some(v) = enabled {
                    rule.enabled = v;
                }
                let (_, updated) = client.upsert_rule(Some(idx), rule, false).await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "index": idx,
                            "rule": updated.config.rules[idx]
                        }))?
                    );
                } else {
                    println!("Updated rule #{idx}: {}", updated.config.rules[idx].name);
                }
                Ok(())
            }
            RuleActionCmd::Move { from, to } => {
                let snap = client.move_rule(from, to).await?;
                if as_json {
                    let order: Vec<_> = snap.config.rules.iter().map(|r| r.name.clone()).collect();
                    println!("{}", serde_json::to_string_pretty(&serde_json::json!({ "ok": true, "order": order }))?);
                } else {
                    println!("Moved rule #{from} -> #{to}.");
                }
                Ok(())
            }
            RuleActionCmd::Toggle { selector, enabled } => {
                let (idx, state, snap) = client.toggle_rule(&selector, enabled).await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "index": idx,
                            "name": snap.config.rules[idx].name,
                            "enabled": state
                        }))?
                    );
                } else {
                    println!("Rule #{idx} ({}) enabled={state}.", snap.config.rules[idx].name);
                }
                Ok(())
            }
            RuleActionCmd::Remove { selector } => {
                let (removed, snap) = client.delete_rule(&selector).await?;
                if as_json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "ok": true,
                            "removed": removed.name,
                            "remaining": snap.config.rules.len()
                        }))?
                    );
                } else {
                    println!("Removed rule '{}'.", removed.name);
                }
                Ok(())
            }
            RuleActionCmd::Test {
                process,
                path,
                host,
                ip,
                port,
                proto,
            } => {
                let report = client
                    .test_rule(RuleTestQuery {
                        process_name: process,
                        exe_path: path,
                        host,
                        dst_ip: ip,
                        dst_port: port,
                        protocol: proto,
                    })
                    .await?;
                if as_json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    println!("Phase 1 (L4) : {} (rule={:?}, reason={})", report.phase1_decision, report.phase1_rule, report.phase1_reason);
                    if let Some(ref d2) = report.phase2_decision {
                        println!("Phase 2 (L7) : {d2} (rule={:?})", report.phase2_rule);
                    }
                    println!(
                        "Final Action : {} (proxy={:?}, matched_rule={:?})",
                        report.final_action, report.final_proxy, report.matched_rule
                    );
                }
                Ok(())
            }
        },
        CtlAction::Connections {
            process,
            state,
            limit,
        } => {
            let p_needle = process.map(|s| s.to_ascii_lowercase());
            let s_needle = state.map(|s| s.to_ascii_lowercase());
            let connections: Vec<_> = client
                .get_connections()
                .await?
                .into_iter()
                .filter(|c| {
                    if let Some(ref p) = p_needle {
                        if !c.process_name.to_ascii_lowercase().contains(p) {
                            return false;
                        }
                    }
                    if let Some(ref s) = s_needle {
                        if format!("{:?}", c.state).to_ascii_lowercase() != *s {
                            return false;
                        }
                    }
                    true
                })
                .take(limit)
                .collect();

            if as_json {
                println!("{}", serde_json::to_string_pretty(&connections)?);
            } else {
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
            }
            Ok(())
        }
        CtlAction::Clear => {
            let message = client.clear_connections().await?;
            if as_json {
                println!("{}", serde_json::json!({ "ok": true, "message": message }));
            } else {
                println!("{message}");
            }
            Ok(())
        }
        CtlAction::Logs { level, limit } => {
            let entries = client.get_logs(level, Some(limit)).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&entries)?);
            } else {
                for e in entries.iter().rev() {
                    println!("[{}] [{}] {}", e.timestamp_ms, e.level, e.message);
                }
            }
            Ok(())
        }
        CtlAction::Update { proxy } => {
            let report = client.check_for_updates(proxy.as_deref()).await?;
            if as_json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else if report.update_available {
                println!(
                    "Update available: v{} -> {} (checked via {})",
                    report.current_version, report.latest_tag, report.checked_via
                );
                println!("Release page : {}", report.release_url);
                if let Some(ref url) = report.primary_asset_url {
                    println!("Download ZIP : {url}");
                }
            } else {
                println!(
                    "Bypath v{} is up to date (latest: {}, checked via {}).",
                    report.current_version, report.latest_tag, report.checked_via
                );
            }
            Ok(())
        }
        CtlAction::Schema => unreachable!(),
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
            "WARN (not elevated - live capture requires Administrator PowerShell or Windows Service)"
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
        println!("      Hint  : Ensure runtime/windivert contains WinDivert.dll and WinDivert64.sys (or run .\\scripts\\fetch-windivert.ps1).");
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
            if report.tcp_reachable && report.handshake_ok.unwrap_or(true) {
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
