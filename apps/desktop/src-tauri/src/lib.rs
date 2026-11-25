use bypath_core::config::{
    default_config_path, is_portable_mode, resolve_windivert_dir, Config, PresetInfo, ProxyConfig,
};
use bypath_core::rules::{RuleTestQuery, RuleTestReport};
use bypath_core::telemetry::{ConnectionRecord, LogEntry};
use bypath_core::update::{
    check_for_updates as core_check_updates, is_safe_release_url, UpdateCheckReport,
};
use bypath_core::upstream::system_proxy::SystemProxyInfo;
use bypath_daemon::{
    install_service, query_service_status, run_as_windows_service, start_service, stop_service,
    uninstall_service, DaemonController, ServiceStateSummary,
};
use bypath_ipc::{
    BackendMode, ConfigSnapshot, DaemonStatus, DoctorReport, IpcClient, IpcHandler, IpcRequest,
    IpcResponse, NamedProxyProbe, ProcessEntry, DEFAULT_PIPE_NAME,
};
use bypath_windivert::is_elevated;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[cfg(windows)]
const FOCUS_PIPE_NAME: &str = r"\\.\pipe\bypath-gui-focus-v1";
#[cfg(windows)]
const SINGLE_INSTANCE_MUTEX: &str = "Local\\BypathDesktopSingleInstanceV1";

/// Allows a standalone `bypath-gui.exe` to host `BypathService` directly when a sibling
/// `bypath-daemon.exe` is not present alongside it.
pub fn maybe_run_service_mode(args: &[String]) -> Option<i32> {
    let first = args.first()?.as_str();
    let mut cfg_path: Option<PathBuf> = None;
    let mut autostart = false;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--config" | "-c" if i + 1 < args.len() => {
                cfg_path = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            "--autostart" => {
                autostart = true;
                i += 1;
            }
            _ => i += 1,
        }
    }

    let res = match first {
        "service-run" => run_as_windows_service(cfg_path, autostart),
        "install" => {
            let exe = std::env::current_exe().ok()?;
            let cfg = cfg_path.unwrap_or_else(default_config_path);
            install_service(&exe, &cfg, true)
        }
        "start" => start_service(),
        "stop" => stop_service(),
        "uninstall" => uninstall_service(),
        _ => return None,
    };

    match res {
        Ok(()) => Some(0),
        Err(_) => Some(1),
    }
}

struct AppBackend {
    ipc: IpcClient,
    embedded: Arc<DaemonController>,
    minimize_to_tray: AtomicBool,
    allow_exit: AtomicBool,
}

impl AppBackend {
    fn new() -> Result<Self, String> {
        let cfg_path = default_config_path();
        let embedded = DaemonController::new(cfg_path, BackendMode::Embedded)
            .map_err(|e| format!("failed to initialize local engine controller: {e}"))?;
        let initial_minimize = embedded.current_config().general.minimize_to_tray;
        Ok(Self {
            ipc: IpcClient::default(),
            embedded,
            minimize_to_tray: AtomicBool::new(initial_minimize),
            allow_exit: AtomicBool::new(false),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrayStatePayload {
    status_label: String,
    show_label: String,
    toggle_label: String,
    quit_label: String,
    tooltip: String,
    minimize_to_tray: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentInfo {
    cli_path: String,
    pipe_name: String,
    config_path: String,
    windivert_dir: String,
    portable_mode: bool,
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
        IpcResponse::Config { snapshot } => {
            state
                .minimize_to_tray
                .store(snapshot.config.general.minimize_to_tray, Ordering::Relaxed);
            Ok(snapshot)
        }
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
    state
        .minimize_to_tray
        .store(config.general.minimize_to_tray, Ordering::Relaxed);
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
async fn get_system_proxy() -> Result<SystemProxyInfo, String> {
    // Always read HKCU Windows Internet Settings in the interactive desktop user session
    // rather than inside the LocalSystem background service.
    Ok(bypath_core::upstream::system_proxy::read_system_proxy())
}

#[tauri::command]
async fn test_rule(
    state: State<'_, Arc<AppBackend>>,
    query: RuleTestQuery,
) -> Result<RuleTestReport, String> {
    match state
        .dispatch(IpcRequest::TestRule {
            query: query.clone(),
        })
        .await?
    {
        IpcResponse::RuleTest { report } => Ok(report),
        IpcResponse::Error { message } if message.contains("unknown variant") => {
            let snap = match state.dispatch(IpcRequest::GetConfig).await? {
                IpcResponse::Config { snapshot } => snapshot,
                IpcResponse::Error { message } => return Err(message),
                other => return Err(format!("unexpected config response: {other:?}")),
            };
            let engine = bypath_core::rules::RuleEngine::from_config(&snap.config)
                .map_err(|e| e.to_string())?;
            Ok(engine.test_query(&query))
        }
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected rule_test response: {other:?}")),
    }
}

#[tauri::command]
async fn get_logs(
    state: State<'_, Arc<AppBackend>>,
    min_level: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<LogEntry>, String> {
    match state
        .dispatch(IpcRequest::GetLogs {
            min_level: min_level.clone(),
            limit,
        })
        .await?
    {
        IpcResponse::Logs { entries } => Ok(entries),
        IpcResponse::Error { message } if message.contains("unknown variant") => Ok(state
            .embedded
            .list_logs(min_level.as_deref(), limit)),
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected logs response: {other:?}")),
    }
}

#[tauri::command]
async fn export_config_toml(state: State<'_, Arc<AppBackend>>) -> Result<String, String> {
    match state.dispatch(IpcRequest::GetConfig).await? {
        IpcResponse::Config { snapshot } => {
            toml::to_string_pretty(&snapshot.config).map_err(|e| e.to_string())
        }
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected config response: {other:?}")),
    }
}

#[tauri::command]
async fn import_config_toml(
    state: State<'_, Arc<AppBackend>>,
    raw_toml: String,
) -> Result<Config, String> {
    let cfg: Config =
        toml::from_str(&raw_toml).map_err(|e| format!("invalid TOML config: {e}"))?;
    cfg.validate().map_err(|e| e.to_string())?;
    match state
        .dispatch(IpcRequest::SaveConfig {
            config: cfg.clone(),
            reload: true,
        })
        .await?
    {
        IpcResponse::Ok { .. } => {
            state
                .minimize_to_tray
                .store(cfg.general.minimize_to_tray, Ordering::Relaxed);
            Ok(cfg)
        }
        IpcResponse::Error { message } => Err(message),
        other => Err(format!("unexpected save_config response: {other:?}")),
    }
}

#[tauri::command]
async fn get_agent_info(state: State<'_, Arc<AppBackend>>) -> Result<AgentInfo, String> {
    let cfg = match state.dispatch(IpcRequest::GetConfig).await? {
        IpcResponse::Config { snapshot } => snapshot,
        _ => ConfigSnapshot {
            path: default_config_path().display().to_string(),
            config: state.embedded.current_config(),
        },
    };
    let cli_path = locate_cli_executable()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "bypath.exe".to_string());
    let wd_dir = resolve_windivert_dir(Some(&cfg.config.general.windivert_dir));
    Ok(AgentInfo {
        cli_path,
        pipe_name: DEFAULT_PIPE_NAME.to_string(),
        config_path: cfg.path,
        windivert_dir: wd_dir.display().to_string(),
        portable_mode: is_portable_mode(),
    })
}

#[tauri::command]
async fn set_tray_state(
    app: AppHandle,
    state: State<'_, Arc<AppBackend>>,
    payload: TrayStatePayload,
) -> Result<(), String> {
    state
        .minimize_to_tray
        .store(payload.minimize_to_tray, Ordering::Relaxed);

    if let Some(tray) = app.tray_by_id("main-tray") {
        let status_item =
            MenuItem::with_id(&app, "status", &payload.status_label, false, None::<&str>)
                .map_err(|e| e.to_string())?;
        let sep1 = PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?;
        let show_item =
            MenuItem::with_id(&app, "show", &payload.show_label, true, None::<&str>)
                .map_err(|e| e.to_string())?;
        let toggle_item =
            MenuItem::with_id(&app, "toggle", &payload.toggle_label, true, None::<&str>)
                .map_err(|e| e.to_string())?;
        let sep2 = PredefinedMenuItem::separator(&app).map_err(|e| e.to_string())?;
        let quit_item =
            MenuItem::with_id(&app, "quit", &payload.quit_label, true, None::<&str>)
                .map_err(|e| e.to_string())?;
        let menu = Menu::with_items(
            &app,
            &[
                &status_item,
                &sep1,
                &show_item,
                &toggle_item,
                &sep2,
                &quit_item,
            ],
        )
        .map_err(|e| e.to_string())?;
        let _ = tray.set_menu(Some(menu));
        let _ = tray.set_tooltip(Some(&payload.tooltip));
    }
    Ok(())
}

#[tauri::command]
async fn open_path_in_explorer(path: String) -> Result<(), String> {
    let target = PathBuf::from(path.trim());
    let dir = if target.is_file() {
        target
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| target.clone())
    } else if target.exists() {
        target
    } else if let Some(parent) = target.parent() {
        if parent.exists() {
            parent.to_path_buf()
        } else {
            let _ = std::fs::create_dir_all(parent);
            parent.to_path_buf()
        }
    } else {
        target
    };

    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(&dir)
            .spawn()
            .map_err(|e| format!("failed to open Explorer: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        Err("Explorer launch is only available on Windows".into())
    }
}

#[tauri::command]
async fn open_windows_security() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd.exe")
            .args(["/C", "start", "", "windowsdefender://threat"])
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("failed to open Windows Security: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        Err("Windows Security is only available on Windows".into())
    }
}

#[tauri::command]
async fn check_for_updates(
    state: State<'_, Arc<AppBackend>>,
) -> Result<UpdateCheckReport, String> {
    let cfg = match state.dispatch(IpcRequest::GetConfig).await {
        Ok(IpcResponse::Config { snapshot }) => Some(snapshot.config),
        _ => None,
    };
    core_check_updates(cfg.as_ref(), None, Duration::from_secs(12))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_external_url(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    if !is_safe_release_url(trimmed) {
        return Err("refused to open unverified external URL".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("cmd.exe")
            .args(["/C", "start", "", trimmed])
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("failed to open browser URL: {e}"))?;
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = trimmed;
        Err("opening external URLs is only implemented on Windows".into())
    }
}

#[tauri::command]
async fn pick_directory(title: Option<String>) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let prompt = title
            .unwrap_or_else(|| "Select WinDivert runtime folder".to_string())
            .replace('\'', "''");
        let ps = format!(
            "Add-Type -AssemblyName System.Windows.Forms; \
             $d = New-Object System.Windows.Forms.FolderBrowserDialog; \
             $d.Description = '{prompt}'; \
             $d.ShowNewFolderButton = $false; \
             if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{ [Console]::Out.Write($d.SelectedPath) }}"
        );
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-STA", "-WindowStyle", "Hidden", "-Command", &ps])
            .output()
            .map_err(|e| format!("failed to open folder picker: {e}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let chosen = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if chosen.is_empty() {
            Ok(None)
        } else {
            Ok(Some(chosen))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        Ok(None)
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
        for cand in &candidates {
            if cand.exists() {
                return Ok(cand.clone());
            }
        }
        // Self-hosted fallback: `bypath-gui.exe` itself links `bypath-daemon` and handles
        // `service-run` / `install` / `start` / `stop` / `uninstall` when downloaded standalone.
        return Ok(exe);
    }
    Err("unable to resolve daemon executable path".into())
}

fn locate_cli_executable() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("bypath.exe")];
    for ancestor in dir.ancestors().skip(1).take(8) {
        candidates.push(ancestor.join("target").join("release").join("bypath.exe"));
        candidates.push(ancestor.join("target").join("debug").join("bypath.exe"));
    }
    candidates.into_iter().find(|p| p.exists())
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

fn reveal_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn setup_tray(app: &AppHandle, backend: Arc<AppBackend>) -> tauri::Result<()> {
    let status_item = MenuItem::with_id(app, "status", "Bypath - Standby", false, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let show_item = MenuItem::with_id(app, "show", "Show Bypath", true, None::<&str>)?;
    let toggle_item = MenuItem::with_id(app, "toggle", "Toggle Proxying", true, None::<&str>)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&status_item, &sep1, &show_item, &toggle_item, &sep2, &quit_item],
    )?;

    let backend_for_menu = Arc::clone(&backend);
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Bypath")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" => reveal_main_window(app),
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
                backend_for_menu.allow_exit.store(true, Ordering::SeqCst);
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
                reveal_main_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg(windows)]
struct SingleInstanceGuard(windows::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
fn acquire_single_instance() -> Option<SingleInstanceGuard> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name_w: Vec<u16> = SINGLE_INSTANCE_MUTEX
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(name_w.as_ptr())) }.ok()?;
    let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already_exists {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(handle);
        }
        notify_existing_instance();
        return None;
    }
    Some(SingleInstanceGuard(handle))
}

#[cfg(windows)]
fn notify_existing_instance() {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .write(true)
        .open(FOCUS_PIPE_NAME)
    {
        let _ = f.write_all(b"focus\n");
        let _ = f.flush();
    }
}

#[cfg(windows)]
fn spawn_focus_listener(app: AppHandle) {
    use tokio::io::AsyncReadExt;
    use tokio::net::windows::named_pipe::ServerOptions;

    tauri::async_runtime::spawn(async move {
        let Ok(mut server) = ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create(FOCUS_PIPE_NAME)
        else {
            return;
        };

        loop {
            if server.connect().await.is_err() {
                break;
            }
            let mut connected = server;
            server = match ServerOptions::new()
                .reject_remote_clients(true)
                .create(FOCUS_PIPE_NAME)
            {
                Ok(next) => next,
                Err(_) => break,
            };

            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                let mut buf = [0u8; 32];
                let _ = connected.read(&mut buf).await;
                reveal_main_window(&app_clone);
            });
        }
    });
}

pub fn run() {
    #[cfg(windows)]
    let _single_instance_guard = match acquire_single_instance() {
        Some(g) => g,
        None => return,
    };

    let backend = Arc::new(AppBackend::new().expect("failed to initialize AppBackend"));

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "bypath_core=info,bypath_windivert=info,bypath_ipc=info,bypath_daemon=info,info",
        )
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_target(false).compact())
        .with(backend.embedded.telemetry().log_layer())
        .try_init();

    let backend_for_close = Arc::clone(&backend);
    let backend_for_exit = Arc::clone(&backend);

    let app = tauri::Builder::default()
        .manage(Arc::clone(&backend))
        .setup(move |app| {
            let handle = app.handle().clone();
            setup_tray(&handle, Arc::clone(&backend))?;
            spawn_event_forwarder(handle.clone(), Arc::clone(&backend));
            #[cfg(windows)]
            spawn_focus_listener(handle);
            Ok(())
        })
        .on_window_event(move |window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let should_hide = !backend_for_close.allow_exit.load(Ordering::SeqCst)
                    && backend_for_close.minimize_to_tray.load(Ordering::Relaxed);
                if should_hide {
                    api.prevent_close();
                    let _ = window.hide();
                }
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
            get_system_proxy,
            test_rule,
            get_logs,
            export_config_toml,
            import_config_toml,
            get_agent_info,
            set_tray_state,
            open_path_in_explorer,
            open_windows_security,
            check_for_updates,
            open_external_url,
            pick_directory,
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
