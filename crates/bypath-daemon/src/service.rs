use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const SERVICE_NAME: &str = "BypathService";
pub const SERVICE_DISPLAY_NAME: &str = "Bypath Transparent Proxy Service";
pub const SERVICE_DESCRIPTION: &str =
    "Per-application transparent proxy engine using WinDivert and userspace TCP/IP netstack.";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStateSummary {
    pub installed: bool,
    pub running: bool,
    pub state: String,
}

#[cfg(windows)]
mod win_impl {
    use super::*;
    use crate::controller::DaemonController;
    use anyhow::{anyhow, Context};
    use bypath_core::config::default_config_path;
    use bypath_ipc::{BackendMode, IpcServer, DEFAULT_PIPE_NAME};
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;
    use tokio::sync::oneshot;
    use tracing::{error, info};
    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl,
        ServiceExitCode, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    use windows_service::{define_windows_service, service_dispatcher};

    struct ServiceBootArgs {
        config_path: PathBuf,
        autostart_engine: bool,
    }

    static BOOT_ARGS: OnceLock<ServiceBootArgs> = OnceLock::new();

    define_windows_service!(ffi_service_main, service_main);

    pub fn query_service_status() -> ServiceStateSummary {
        let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        else {
            return ServiceStateSummary {
                installed: false,
                running: false,
                state: "unknown".into(),
            };
        };

        let Ok(service) = manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) else {
            return ServiceStateSummary {
                installed: false,
                running: false,
                state: "not_installed".into(),
            };
        };

        match service.query_status() {
            Ok(status) => {
                let (running, state) = match status.current_state {
                    ServiceState::Running => (true, "running"),
                    ServiceState::StartPending => (false, "start_pending"),
                    ServiceState::StopPending => (false, "stop_pending"),
                    ServiceState::Stopped => (false, "stopped"),
                    ServiceState::Paused => (false, "paused"),
                    ServiceState::PausePending => (false, "pause_pending"),
                    ServiceState::ContinuePending => (false, "continue_pending"),
                };
                ServiceStateSummary {
                    installed: true,
                    running,
                    state: state.into(),
                }
            }
            Err(_) => ServiceStateSummary {
                installed: true,
                running: false,
                state: "unknown".into(),
            },
        }
    }

    pub fn install_service(
        daemon_exe: &Path,
        config_path: &Path,
        autostart_boot: bool,
    ) -> Result<()> {
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )
        .context("failed to open Windows Service Control Manager (Administrator privileges required)")?;

        let start_type = if autostart_boot {
            ServiceStartType::AutoStart
        } else {
            ServiceStartType::OnDemand
        };

        let launch_arguments = vec![
            OsString::from("service-run"),
            OsString::from("--config"),
            config_path.as_os_str().to_os_string(),
        ];

        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type,
            error_control: ServiceErrorControl::Normal,
            executable_path: daemon_exe.to_path_buf(),
            launch_arguments,
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };

        let service = manager
            .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
            .context("failed to create BypathService (is it already installed?)")?;
        let _ = service.set_description(SERVICE_DESCRIPTION);
        Ok(())
    }

    pub fn uninstall_service() -> Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
                .context("failed to open Service Control Manager (Administrator required)")?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::STOP | ServiceAccess::QUERY_STATUS | ServiceAccess::DELETE,
            )
            .context("BypathService is not installed or access was denied")?;

        if let Ok(status) = service.query_status() {
            if status.current_state != ServiceState::Stopped {
                let _ = service.stop();
                for _ in 0..20 {
                    std::thread::sleep(Duration::from_millis(250));
                    if let Ok(s) = service.query_status() {
                        if s.current_state == ServiceState::Stopped {
                            break;
                        }
                    }
                }
            }
        }

        service
            .delete()
            .context("failed to delete BypathService")?;
        Ok(())
    }

    pub fn start_service() -> Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
                .context("failed to open Service Control Manager (Administrator required)")?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::START | ServiceAccess::QUERY_STATUS,
            )
            .context("BypathService is not installed or access was denied")?;

        if let Ok(st) = service.query_status() {
            if st.current_state == ServiceState::Running {
                return Ok(());
            }
        }

        service
            .start::<&str>(&[])
            .context("failed to start BypathService")?;
        Ok(())
    }

    pub fn stop_service() -> Result<()> {
        let manager =
            ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
                .context("failed to open Service Control Manager (Administrator required)")?;
        let service = manager
            .open_service(
                SERVICE_NAME,
                ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
            )
            .context("BypathService is not installed or access was denied")?;

        if let Ok(st) = service.query_status() {
            if st.current_state == ServiceState::Stopped {
                return Ok(());
            }
        }

        service
            .stop()
            .context("failed to stop BypathService")?;
        Ok(())
    }

    pub fn run_as_windows_service(
        config_path: Option<PathBuf>,
        autostart_engine: bool,
    ) -> Result<()> {
        let _ = BOOT_ARGS.set(ServiceBootArgs {
            config_path: config_path.unwrap_or_else(default_config_path),
            autostart_engine,
        });
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
            .map_err(|err| anyhow!("Windows Service dispatcher failed: {err}"))
    }

    fn service_main(_arguments: Vec<OsString>) {
        if let Err(err) = run_service_inner() {
            error!(error = %err, "BypathService terminated with error");
        }
    }

    fn run_service_inner() -> Result<()> {
        let (stop_tx, stop_rx) = oneshot::channel::<()>();
        let stop_slot = Mutex::new(Some(stop_tx));

        let status_handle =
            service_control_handler::register(SERVICE_NAME, move |control_event| {
                match control_event {
                    ServiceControl::Stop | ServiceControl::Shutdown => {
                        if let Ok(mut guard) = stop_slot.lock() {
                            if let Some(tx) = guard.take() {
                                let _ = tx.send(());
                            }
                        }
                        ServiceControlHandlerResult::NoError
                    }
                    ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                    _ => ServiceControlHandlerResult::NotImplemented,
                }
            })?;

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::StartPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })?;

        let boot = BOOT_ARGS.get();
        let config_path = boot
            .map(|b| b.config_path.clone())
            .unwrap_or_else(default_config_path);
        let autostart_engine = boot.map(|b| b.autostart_engine).unwrap_or(false);

        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;

        rt.block_on(async {
            let controller = DaemonController::new(config_path, BackendMode::Service)?;
            let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(
                |_| {
                    tracing_subscriber::EnvFilter::new(
                        "bypath_core=info,bypath_windivert=info,bypath_ipc=info,bypath_daemon=info,info",
                    )
                },
            );
            use tracing_subscriber::layer::SubscriberExt;
            use tracing_subscriber::util::SubscriberInitExt;
            let _ = tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().with_target(false).compact())
                .with(controller.telemetry().log_layer())
                .try_init();

            let should_autostart =
                autostart_engine || controller.current_config().general.auto_start_engine;
            if should_autostart {
                if let Err(err) = controller.start_engine().await {
                    error!(error = %err, "autostart engine failed during service startup");
                }
            }

            let (ipc_shutdown_tx, ipc_shutdown_rx) = oneshot::channel();
            let ipc_server = IpcServer::new(DEFAULT_PIPE_NAME, controller.clone());
            let ipc_task = tokio::spawn(async move {
                if let Err(err) = ipc_server.run(ipc_shutdown_rx).await {
                    error!(error = %err, "IPC server exited with error");
                }
            });

            status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::Running,
                controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::default(),
                process_id: None,
            })?;
            info!("BypathService is running");

            let _ = stop_rx.await;

            let _ = status_handle.set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: ServiceState::StopPending,
                controls_accepted: ServiceControlAccept::empty(),
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint: Duration::from_secs(5),
                process_id: None,
            });

            let _ = controller.stop_engine().await;
            let _ = ipc_shutdown_tx.send(());
            let _ = ipc_task.await;
            Ok::<(), anyhow::Error>(())
        })?;

        status_handle.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;
        Ok(())
    }
}

#[cfg(not(windows))]
mod win_impl {
    use super::*;
    use anyhow::bail;

    pub fn query_service_status() -> ServiceStateSummary {
        ServiceStateSummary {
            installed: false,
            running: false,
            state: "unsupported".into(),
        }
    }

    pub fn install_service(_: &Path, _: &Path, _: bool) -> Result<()> {
        bail!("Windows Service management is only available on Windows")
    }

    pub fn uninstall_service() -> Result<()> {
        bail!("Windows Service management is only available on Windows")
    }

    pub fn start_service() -> Result<()> {
        bail!("Windows Service management is only available on Windows")
    }

    pub fn stop_service() -> Result<()> {
        bail!("Windows Service management is only available on Windows")
    }

    pub fn run_as_windows_service(_: Option<PathBuf>, _: bool) -> Result<()> {
        bail!("Windows Service runtime is only available on Windows")
    }
}

pub use win_impl::{
    install_service, query_service_status, run_as_windows_service, start_service, stop_service,
    uninstall_service,
};
