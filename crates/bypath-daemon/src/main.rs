use anyhow::Result;
use bypath_core::config::default_config_path;
use bypath_daemon::{
    install_service, query_service_status, run_as_windows_service, start_service, stop_service,
    uninstall_service, DaemonController, SERVICE_NAME,
};
use bypath_ipc::{BackendMode, IpcServer, DEFAULT_PIPE_NAME};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::sync::oneshot;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "bypath-daemon",
    version,
    about = "Background Windows Service and Named Pipe IPC daemon for Bypath"
)]
struct DaemonCli {
    #[arg(long, global = true, default_value = "info")]
    log_level: String,

    #[command(subcommand)]
    command: DaemonCommand,
}

#[derive(Subcommand, Debug)]
enum DaemonCommand {
    /// Run the IPC daemon in the foreground (Ctrl+C to exit)
    Run {
        #[arg(long, short)]
        config: Option<PathBuf>,

        #[arg(long, default_value = DEFAULT_PIPE_NAME)]
        pipe: String,

        /// Immediately start transparent packet interception on launch
        #[arg(long)]
        autostart: bool,
    },

    /// Entry point invoked by Windows Service Control Manager (SCM)
    ServiceRun {
        #[arg(long, short)]
        config: Option<PathBuf>,

        #[arg(long)]
        autostart: bool,
    },

    /// Install BypathService into Windows Service Control Manager
    Install {
        #[arg(long, short)]
        config: Option<PathBuf>,

        /// Configure manual/on-demand startup instead of automatic boot startup
        #[arg(long)]
        demand_start: bool,
    },

    /// Stop and remove BypathService
    Uninstall,

    /// Start BypathService via Windows SCM
    Start,

    /// Stop BypathService via Windows SCM
    Stop,

    /// Show current Windows Service installation and running state
    Status,
}

fn main() -> Result<()> {
    let cli = DaemonCli::parse();
    init_tracing(&cli.log_level);

    match cli.command {
        DaemonCommand::ServiceRun { config, autostart } => {
            run_as_windows_service(config, autostart)
        }
        DaemonCommand::Install {
            config,
            demand_start,
        } => {
            let exe = std::env::current_exe()?;
            let cfg_path = config.unwrap_or_else(default_config_path);
            install_service(&exe, &cfg_path, !demand_start)?;
            println!(
                "Installed Windows Service '{SERVICE_NAME}' using config {}",
                cfg_path.display()
            );
            Ok(())
        }
        DaemonCommand::Uninstall => {
            uninstall_service()?;
            println!("Uninstalled Windows Service '{SERVICE_NAME}'.");
            Ok(())
        }
        DaemonCommand::Start => {
            start_service()?;
            println!("Started Windows Service '{SERVICE_NAME}'.");
            Ok(())
        }
        DaemonCommand::Stop => {
            stop_service()?;
            println!("Stopped Windows Service '{SERVICE_NAME}'.");
            Ok(())
        }
        DaemonCommand::Status => {
            let s = query_service_status();
            println!(
                "Service '{SERVICE_NAME}': installed={} running={} state={}",
                s.installed, s.running, s.state
            );
            Ok(())
        }
        DaemonCommand::Run {
            config,
            pipe,
            autostart,
        } => {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()?;
            rt.block_on(async move {
                let cfg_path = config.unwrap_or_else(default_config_path);
                let controller = DaemonController::new(cfg_path, BackendMode::Service)?;

                if autostart {
                    if let Err(err) = controller.start_engine().await {
                        warn!(error = %err, "autostart engine failed; daemon remains online for IPC");
                    }
                }

                let (shutdown_tx, shutdown_rx) = oneshot::channel();
                tokio::spawn(async move {
                    if tokio::signal::ctrl_c().await.is_ok() {
                        let _ = shutdown_tx.send(());
                    }
                });

                info!(pipe = %pipe, "starting bypath foreground daemon");
                let server = IpcServer::new(pipe, controller.clone());
                server.run(shutdown_rx).await?;
                controller.stop_engine().await?;
                Ok(())
            })
        }
    }
}

fn init_tracing(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "bypath_core={level},bypath_windivert={level},bypath_ipc={level},bypath_daemon={level},{level}"
        ))
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .init();
}
