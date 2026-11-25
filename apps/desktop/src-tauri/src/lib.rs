use bypath_core::config::{default_config_path, Config, PresetInfo, ProxyConfig};
use bypath_core::telemetry::ConnectionRecord;
use bypath_daemon::{
    install_service, query_service_status, start_service, stop_service, uninstall_service,
    DaemonController, ServiceStateSummary,
};
use bypath_ipc::{
    BackendMode, ConfigSnapshot, DaemonStatus, DoctorReport, IpcClient, IpcHandler, IpcRequest,
    IpcResponse, NamedProxyProbe, ProcessEntry,
};
use bypath_windivert::is_elevated;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

struct AppBackend {
    ipc: IpcClient,
    embedded: Arc<DaemonController>,
}

impl AppBackend {
    fn new() -> Result<Self, String> {
        let cfg_path = default_config_path();
        let embedded = DaemonController::new(cfg_path, BackendMode::Embedded)
            .map_err(|e| format!("failed to initialize local engine controller: {e}"))?;
        Ok(Self {
            ipc: IpcClient::default(),
            embedded,
        })
    }

    async fn is_service_pipe_live(&self) -> bool {
        matches!(
            self.ipc.request(&IpcRequest::Ping).await,
            Ok(IpcResponse::Pong { .. })
        )
    }

    /// Dispatches a request to the background Windows Service / Named Pipe daemon if online,
    /// transparently falling back to the embedded local controller otherwise.
    async fn dispatch(&self, req: IpcRequest) -> Result<IpcResponse, String> {
        if let Ok(resp) = self.ipc.request(&req).await {
            return Ok(resp);
        }
        Ok(self.embedded.handle(req).await)
    }
}

#[tauri::command]
async fn get_status(state: State<'_, Arc<AppBackend>>) -> Result<DaemonStatus, String> {
    match state.dispatch(IpcRequest::GetStatus).await? {
        IpcResponse::Status { status } => Ok(status),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected status response: {other:?}")),
    }
}

#[tauri::command]
async fn get_config(state: State<'_, Arc<AppBackend>>) -> Result<ConfigSnapshot, String> {
    match state.dispatch(IpcRequest::GetConfig).await? {
        IpcResponse::Config { snapshot } => Ok(snapshot),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected config response: {other:?}")),
    }
}

#[tauri::command]
async fn save_config(
    state: State<'_, Arc<AppBackend>>,
    config: Config,
    reload: Option<bool>,
) -> Result<String, String> {
    let req = IpcRequest::SaveConfig {
        config,
        reload: reload.unwrap_or(true),
    };
    match state.dispatch(req).await? {
        IpcResponse::Ok { message } => Ok(message),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected save_config response: {other:?}")),
    }
}

#[tauri::command]
async fn start_engine(state: State<'_, Arc<AppBackend>>) -> Result<String, String> {
    match state.dispatch(IpcRequest::StartEngine).await? {
        IpcResponse::Ok { message } => Ok(message),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected start_engine response: {other:?}")),
    }
}

#[tauri::command]
async fn stop_engine(state: State<'_, Arc<AppBackend>>) -> Result<String, String> {
    match state.dispatch(IpcRequest::StopEngine).await? {
        IpcResponse::Ok { message } => Ok(message),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected stop_engine response: {other:?}")),
    }
}

#[tauri::command]
async fn get_connections(state: State<'_, Arc<AppBackend>>) -> Result<Vec<ConnectionRecord>, String> {
    match state.dispatch(IpcRequest::GetConnections).await? {
        IpcResponse::Connections { connections } => Ok(connections),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected connections response: {other:?}")),
    }
}

#[tauri::command]
async fn clear_connections(state: State<'_, Arc<AppBackend>>) -> Result<String, String> {
    match state.dispatch(IpcRequest::ClearConnections).await? {
        IpcResponse::Ok { message } => Ok(message),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected clear_connections response: {other:?}")),
    }
}

#[tauri::command]
async fn list_processes(
    state: State<'_, Arc<AppBackend>>,
    filter: Option<String>,
) -> Result<Vec<ProcessEntry>, String> {
    match state.dispatch(IpcRequest::ListProcesses { filter }).await? {
        IpcResponse::Processes { processes } => Ok(processes),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected list_processes response: {other:?}")),
    }
}

#[tauri::command]
async fn run_doctor(state: State<'_, Arc<AppBackend>>) -> Result<DoctorReport, String> {
    match state.dispatch(IpcRequest::RunDoctor).await? {
        IpcResponse::Doctor { report } => Ok(report),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected doctor response: {other:?}")),
    }
}

#[tauri::command]
async fn probe_proxy_endpoint(
    state: State<'_, Arc<AppBackend>>,
    proxy: ProxyConfig,
) -> Result<NamedProxyProbe, String> {
    match state.dispatch(IpcRequest::ProbeProxy { proxy }).await? {
        IpcResponse::ProxyProbe { probe } => Ok(probe),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected probe response: {other:?}")),
    }
}

#[tauri::command]
async fn list_presets(state: State<'_, Arc<AppBackend>>) -> Result<Vec<PresetInfo>, String> {
    match state.dispatch(IpcRequest::ListPresets).await? {
        IpcResponse::Presets { presets } => Ok(presets),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected presets response: {other:?}")),
    }
}

#[tauri::command]
async fn get_service_status() -> Result<ServiceStateSummary, String> {
    Ok(query_service_status())
}

#[tauri::command]
async fn manage_windows_service(action: String) -> Result<ServiceStateSummary, String> {
    let daemon_exe = locate_daemon_executable()?;
    let cfg_path = default_config_path();

    if is_elevated() {
        match action.as_str() {
            "install" => {
                install_service(&daemon_exe, &cfg_path, true).map_err(|e| e.to_string())?;
                start_service().map_err(|e| e.to_string())?;
            }
            "start" => start_service().map_err(|e| e.to_string())?,
            "stop" => stop_service().map_err(|e| e.to_string())?,
            "uninstall" => uninstall_service().map_err(|e| e.to_string())?,
            other => return Err(format!("unsupported service action '{other}'")),
        }
    } else {
        #[cfg(windows)]
        {
            run_elevated_daemon_action(&daemon_exe, &cfg_path, &action)?;
        }
        #[cfg(not(windows))]
        {
            return Err("Windows Service management is only supported on Windows".into());
        }
    }

    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(query_service_status())
}

fn locate_daemon_executable() -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("bypath-daemon.exe"));
            for ancestor in dir.ancestors().skip(1).take(8) {
                candidates.push(
                    ancestor
                        .join("target")
                        .join("release")
                        .join("bypath-daemon.exe"),
                );
                candidates.push(
                    ancestor
                        .join("target")
                        .join("debug")
                        .join("bypath-daemon.exe"),
                );
            }
        }
    }
    for cand in &candidates {
        if cand.exists() {
            return Ok(cand.clone());
        }
    }
    Err(
        "bypath-daemon.exe was not found. Build the workspace (`cargo build --release`) first."
            .into(),
    )
}

#[cfg(windows)]
fn run_elevated_daemon_action(
    daemon_exe: &Path,
    cfg_path: &Path,
    action: &str,
) -> Result<(), String> {
    let exe_str = daemon_exe.display().to_string().replace('\'', "''");
    let cfg_str = cfg_path.display().to_string().replace('\'', "''");
    let script = match action {
        "install" => format!(
            "& '{exe_str}' install --config '{cfg_str}'; if ($LASTEXITCODE -eq 0) {{ & '{exe_str}' start }}"
        ),
        "start" => format!("& '{exe_str}' start"),
        "stop" => format!("& '{exe_str}' stop"),
        "uninstall" => format!("& '{exe_str}' uninstall"),
        other => return Err(format!("unsupported service action '{other}'")),
    };

    let ps_cmd = format!(
        "Start-Process -FilePath 'powershell.exe' -Verb RunAs -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-Command',\"{}\"",
        script.replace('"', "`\"")
    );

    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps_cmd])
        .status()
        .map_err(|e| format!("failed to launch elevated service command: {e}"))?;

    if !status.success() {
        return Err("elevated service action was cancelled or failed".into());
    }
    Ok(())
}

fn spawn_event_forwarder(app: AppHandle, backend: Arc<AppBackend>) {
    tauri::async_runtime::spawn(async move {
        let mut embedded_rx = backend.embedded.telemetry().subscribe();
        loop {
            if backend.is_service_pipe_live().await {
                if let Ok(mut pipe_rx) = backend.ipc.subscribe_events().await {
                    info!("GUI event stream attached to background service Named Pipe");
                    while let Some(evt) = pipe_rx.recv().await {
                        let _ = app.emit("bypath://event", &evt);
                    }
                    warn!("background service event stream disconnected; falling back");
                }
            }

            // Wait for either an embedded telemetry event or a 2-second service re-probe.
            tokio::select! {
                res = embedded_rx.recv() => {
                    match res {
                        Ok(evt) => {
                            let _ = app.emit("bypath://event", &evt);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            }
        }
    });
}

fn setup_tray(app: &AppHandle, backend: Arc<AppBackend>) -> tauri::Result<()> {
    let show_item = MenuItem::with_id(app, "show", "Show Bypath", true, None::<&str>)?;
    let toggle_item =
        MenuItem::with_id(app, "toggle", "Toggle Proxying", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_item, &toggle_item, &sep, &quit_item])?;

    let backend_for_menu = Arc::clone(&backend);
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Bypath")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
            "toggle" => {
                let b = Arc::clone(&backend_for_menu);
                let app_handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Ok(IpcResponse::Status { status }) =
                        b.dispatch(IpcRequest::GetStatus).await
                    {
                        let req = if status.engine_running {
                            IpcRequest::StopEngine
                        } else {
                            IpcRequest::StartEngine
                        };
                        let _ = b.dispatch(req).await;
                        let _ = app_handle.emit("bypath://refresh", ());
                    }
                });
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.show();
                    let _ = win.unminimize();
                    let _ = win.set_focus();
                }
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn run() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("bypath_core=info,bypath_windivert=info,bypath_ipc=info,bypath_daemon=info,info")
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .compact()
        .try_init();

    let backend = Arc::new(AppBackend::new().expect("failed to initialize AppBackend"));
    let backend_for_exit = Arc::clone(&backend);

    let app = tauri::Builder::default()
        .manage(Arc::clone(&backend))
        .setup(move |app| {
            let handle = app.handle().clone();
            setup_tray(&handle, Arc::clone(&backend))?;
            spawn_event_forwarder(handle, Arc::clone(&backend));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { .. } = event {
                // Keep clean predictable exit when user clicks window close button.
                let _ = window;
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_config,
            save_config,
            start_engine,
            stop_engine,
            get_connections,
            clear_connections,
            list_processes,
            run_doctor,
            probe_proxy_endpoint,
            list_presets,
            get_service_status,
            manage_windows_service,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Bypath Tauri application");

    app.run(move |_app_handle, event| {
        if let RunEvent::Exit = event {
            // Release embedded WinDivert handles cleanly on exit (external service stays running).
            let embedded = Arc::clone(&backend_for_exit.embedded);
            tauri::async_runtime::block_on(async move {
                let _ = embedded.stop_engine().await;
            });
        }
    });
}
