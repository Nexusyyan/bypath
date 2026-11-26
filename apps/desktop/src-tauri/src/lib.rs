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
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, RunEvent, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tokio::sync::watch;
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const TRAY_WINDOW_LABEL: &str = "tray-menu";
const TRAY_DEFAULT_WIDTH: f64 = 288.0;
const TRAY_DEFAULT_HEIGHT: f64 = 456.0;

#[cfg(windows)]
const FOCUS_PIPE_NAME: &str = r"\\.\pipe\bypath-gui-focus-v1";
#[cfg(windows)]
const SINGLE_INSTANCE_MUTEX: &str = "Local\\BypathDesktopSingleInstanceV1";
#[cfg(windows)]
const STARTUP_RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const STARTUP_APPROVED_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
#[cfg(windows)]
const STARTUP_VALUE_NAME: &str = "Bypath";

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct LaunchOptions {
    start_foreground: bool,
    handoff: bool,
    minimized: bool,
    audit_instance: bool,
}

impl LaunchOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = Self::default();
        for arg in args {
            match arg.as_str() {
                "--start-foreground" => opts.start_foreground = true,
                "--handoff" => opts.handoff = true,
                "--minimized" => opts.minimized = true,
                "--audit-instance" => opts.audit_instance = true,
                _ => {}
            }
        }
        opts
    }
}

struct AppBackend {
    ipc: IpcClient,
    embedded: Arc<DaemonController>,
    active_backend: RwLock<BackendMode>,
    mode_tx: watch::Sender<BackendMode>,
    minimize_to_tray: AtomicBool,
    allow_exit: AtomicBool,
    #[cfg(windows)]
    single_instance_guard: Mutex<Option<SingleInstanceGuard>>,
    tray_last_blur: Mutex<Option<Instant>>,
    tray_last_shown: Mutex<Option<Instant>>,
    tray_last_anchor: Mutex<Option<(f64, f64)>>,
}

impl AppBackend {
    fn new(
        initial_mode: BackendMode,
        #[cfg(windows)] single_instance_guard: Option<SingleInstanceGuard>,
    ) -> Result<Self, String> {
        let cfg_path = default_config_path();
        let embedded = DaemonController::new(cfg_path, BackendMode::Embedded)
            .map_err(|e| format!("failed to initialize local engine controller: {e}"))?;
        let initial_minimize = embedded.current_config().general.minimize_to_tray;
        let (mode_tx, _) = watch::channel(initial_mode);
        Ok(Self {
            ipc: IpcClient::default(),
            embedded,
            active_backend: RwLock::new(initial_mode),
            mode_tx,
            minimize_to_tray: AtomicBool::new(initial_minimize),
            allow_exit: AtomicBool::new(false),
            #[cfg(windows)]
            single_instance_guard: Mutex::new(single_instance_guard),
            tray_last_blur: Mutex::new(None),
            tray_last_shown: Mutex::new(None),
            tray_last_anchor: Mutex::new(None),
        })
    }

    fn current_backend(&self) -> BackendMode {
        self.active_backend
            .read()
            .map(|g| *g)
            .unwrap_or(BackendMode::Embedded)
    }

    fn set_active_backend(&self, mode: BackendMode) {
        if let Ok(mut guard) = self.active_backend.write() {
            if *guard != mode {
                *guard = mode;
                let _ = self.mode_tx.send(mode);
            }
        }
    }

    fn subscribe_mode(&self) -> watch::Receiver<BackendMode> {
        self.mode_tx.subscribe()
    }

    #[cfg(windows)]
    fn release_single_instance(&self) {
        if let Ok(mut guard) = self.single_instance_guard.lock() {
            *guard = None;
        }
    }

    async fn is_service_pipe_live(&self) -> bool {
        matches!(
            self.ipc.request(&IpcRequest::Ping).await,
            Ok(IpcResponse::Pong { .. })
        )
    }

    async fn wait_for_service_pipe(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.is_service_pipe_live().await {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Dispatches requests to the active runtime mode:
    /// - Pinned to `embedded` whenever foreground mode is actively running.
    /// - Otherwise routes to `BypathService` over Named Pipe when online, falling back
    ///   cleanly to `embedded` (reloading disk config on fallback).
    async fn dispatch(&self, req: IpcRequest) -> Result<IpcResponse, String> {
        let current = self.current_backend();
        if current == BackendMode::Embedded && self.embedded.is_engine_running() {
            return Ok(self.embedded.handle(req).await);
        }
        if let Ok(resp) = self.ipc.request(&req).await {
            if current != BackendMode::Service {
                self.set_active_backend(BackendMode::Service);
            }
            return Ok(resp);
        }
        if current != BackendMode::Embedded {
            let _ = self.embedded.reload_config_from_disk();
            self.set_active_backend(BackendMode::Embedded);
        }
        Ok(self.embedded.handle(req).await)
    }

    async fn current_status(&self) -> Result<DaemonStatus, String> {
        match self.dispatch(IpcRequest::GetStatus).await? {
            IpcResponse::Status { status } => Ok(status),
            IpcResponse::Error { message } => Err(message),
            other => Err(format!("unexpected status response: {other:?}")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SetRunModeResult {
    mode: String,
    elevating: bool,
    status: DaemonStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrayStatePayload {
    tooltip: String,
    #[serde(default)]
    minimize_to_tray: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UiPrefsSyncPayload {
    locale: Option<String>,
    theme: Option<String>,
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
    if let Some(minimize) = payload.minimize_to_tray {
        state.minimize_to_tray.store(minimize, Ordering::Relaxed);
    }

    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(Some(&payload.tooltip));
    }
    Ok(())
}

#[tauri::command]
async fn open_main_window(app: AppHandle, tab: Option<String>) -> Result<(), String> {
    hide_tray_window(&app);
    reveal_main_window(&app);
    if let Some(target_tab) = tab {
        let _ = app.emit("bypath://navigate", target_tab);
    }
    Ok(())
}

#[tauri::command]
async fn hide_tray_menu(app: AppHandle) -> Result<(), String> {
    hide_tray_window(&app);
    Ok(())
}

#[tauri::command]
async fn resize_tray_menu(
    app: AppHandle,
    state: State<'_, Arc<AppBackend>>,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let clamped_w = width.clamp(220.0, 400.0).ceil();
    let clamped_h = height.clamp(160.0, 640.0).ceil();
    if let Some(win) = app.get_webview_window(TRAY_WINDOW_LABEL) {
        let _ = win.set_size(LogicalSize::new(clamped_w, clamped_h));
        if win.is_visible().unwrap_or(false) {
            let anchor = state.tray_last_anchor.lock().ok().and_then(|g| *g);
            if let Some((ax, ay)) = anchor {
                position_tray_window(&app, &win, ax, ay);
            }
        }
    }
    Ok(())
}

#[tauri::command]
async fn sync_ui_prefs(
    app: AppHandle,
    locale: Option<String>,
    theme: Option<String>,
) -> Result<(), String> {
    let _ = app.emit("bypath://ui-prefs", UiPrefsSyncPayload { locale, theme });
    Ok(())
}

#[tauri::command]
async fn quit_app(app: AppHandle, state: State<'_, Arc<AppBackend>>) -> Result<(), String> {
    state.allow_exit.store(true, Ordering::SeqCst);
    hide_tray_window(&app);
    app.exit(0);
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
async fn set_run_mode(
    app: AppHandle,
    state: State<'_, Arc<AppBackend>>,
    mode: String,
) -> Result<SetRunModeResult, String> {
    match mode.as_str() {
        "off" => {
            let _ = state.embedded.stop_engine().await;
            if state.is_service_pipe_live().await {
                let _ = state.ipc.request(&IpcRequest::StopEngine).await;
                state.set_active_backend(BackendMode::Service);
            } else {
                state.set_active_backend(BackendMode::Embedded);
            }
            let _ = app.emit("bypath://refresh", ());
            let status = state.current_status().await?;
            Ok(SetRunModeResult {
                mode: "off".into(),
                elevating: false,
                status,
            })
        }
        "foreground" => {
            if !is_elevated() {
                #[cfg(windows)]
                {
                    let exe = std::env::current_exe()
                        .map_err(|e| format!("failed to resolve current executable: {e}"))?;
                    spawn_elevated_foreground(&exe)?;
                    let status = state.current_status().await?;
                    let app_clone = app.clone();
                    let backend_clone = Arc::clone(state.inner());
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(80)).await;
                        backend_clone.allow_exit.store(true, Ordering::SeqCst);
                        hide_tray_window(&app_clone);
                        if let Some(win) = app_clone.get_webview_window("main") {
                            let _ = win.hide();
                        }
                        backend_clone.release_single_instance();
                        app_clone.exit(0);
                    });
                    return Ok(SetRunModeResult {
                        mode: "foreground".into(),
                        elevating: true,
                        status,
                    });
                }
                #[cfg(not(windows))]
                {
                    return Err("Foreground elevation is only supported on Windows".into());
                }
            }

            if state.is_service_pipe_live().await {
                if let Ok(IpcResponse::Status { status }) =
                    state.ipc.request(&IpcRequest::GetStatus).await
                {
                    if status.engine_running {
                        let _ = state.ipc.request(&IpcRequest::StopEngine).await;
                        tokio::time::sleep(Duration::from_millis(150)).await;
                    }
                }
            }

            let _ = state.embedded.reload_config_from_disk();
            state.set_active_backend(BackendMode::Embedded);
            state
                .embedded
                .start_engine()
                .await
                .map_err(|e| e.to_string())?;
            let _ = app.emit("bypath://refresh", ());
            let status = state.current_status().await?;
            Ok(SetRunModeResult {
                mode: "foreground".into(),
                elevating: false,
                status,
            })
        }
        "background" => {
            let was_embedded_running = state.embedded.is_engine_running();
            let embedded_cfg = if was_embedded_running {
                Some(state.embedded.current_config())
            } else {
                None
            };
            if was_embedded_running {
                let _ = state.embedded.stop_engine().await;
                tokio::time::sleep(Duration::from_millis(150)).await;
            }

            if !state.is_service_pipe_live().await {
                let svc = query_service_status();
                let action = if svc.installed { "start" } else { "install" };
                execute_service_action(action)?;
                if !state.wait_for_service_pipe(Duration::from_secs(8)).await {
                    return Err("SERVICE_PIPE_TIMEOUT".into());
                }
            }

            if let Some(cfg) = embedded_cfg {
                let _ = state
                    .ipc
                    .request(&IpcRequest::SaveConfig {
                        config: cfg,
                        reload: false,
                    })
                    .await;
            }

            state.set_active_backend(BackendMode::Service);
            match state
                .ipc
                .request(&IpcRequest::StartEngine)
                .await
                .map_err(|e| e.to_string())?
            {
                IpcResponse::Ok { .. } => {}
                IpcResponse::Error { message } => return Err(message),
                other => return Err(format!("unexpected start_engine response: {other:?}")),
            }

            let _ = app.emit("bypath://refresh", ());
            let status = state.current_status().await?;
            Ok(SetRunModeResult {
                mode: "background".into(),
                elevating: false,
                status,
            })
        }
        other => Err(format!("unsupported run mode '{other}'")),
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn format_startup_command(exe_path: &Path) -> String {
    format!("\"{}\" --minimized", exe_path.display())
}

#[cfg_attr(not(windows), allow(dead_code))]
fn is_startup_approved_bytes(bytes: &[u8]) -> bool {
    // Windows StartupApproved\Run uses an even leading byte (typically 0x02) for enabled
    // and an odd leading byte (typically 0x03) when disabled via Task Manager.
    bytes.first().map(|b| (*b & 1) == 0).unwrap_or(true)
}

#[cfg(windows)]
fn read_launch_at_login_registry() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ,
        REG_BINARY, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };

    let run_subkey = to_wide_null(STARTUP_RUN_SUBKEY);
    let mut run_key = HKEY::default();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(run_subkey.as_ptr()),
            0,
            KEY_READ,
            &mut run_key,
        )
    }
    .is_err()
    {
        return false;
    }

    let val_name = to_wide_null(STARTUP_VALUE_NAME);
    let mut kind = REG_VALUE_TYPE::default();
    let mut byte_len = 0u32;
    let exists = unsafe {
        RegQueryValueExW(
            run_key,
            PCWSTR(val_name.as_ptr()),
            None,
            Some(&mut kind),
            None,
            Some(&mut byte_len),
        )
    }
    .is_ok()
        && (kind == REG_SZ || kind == REG_EXPAND_SZ)
        && byte_len > 2;
    unsafe {
        let _ = RegCloseKey(run_key);
    }
    if !exists {
        return false;
    }

    let approved_subkey = to_wide_null(STARTUP_APPROVED_SUBKEY);
    let mut approved_key = HKEY::default();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(approved_subkey.as_ptr()),
            0,
            KEY_READ,
            &mut approved_key,
        )
    }
    .is_ok()
    {
        let mut akind = REG_VALUE_TYPE::default();
        let mut buf = [0u8; 32];
        let mut len = buf.len() as u32;
        let ok = unsafe {
            RegQueryValueExW(
                approved_key,
                PCWSTR(val_name.as_ptr()),
                None,
                Some(&mut akind),
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            )
        }
        .is_ok();
        unsafe {
            let _ = RegCloseKey(approved_key);
        }
        if ok && akind == REG_BINARY && len > 0 {
            return is_startup_approved_bytes(&buf[..len as usize]);
        }
    }

    true
}

#[cfg(windows)]
fn delete_registry_value_best_effort(subkey: &str, value_name: &str) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };

    let subkey_w = to_wide_null(subkey);
    let mut hkey = HKEY::default();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_SET_VALUE,
            &mut hkey,
        )
    }
    .is_ok()
    {
        let val_w = to_wide_null(value_name);
        unsafe {
            let _ = RegDeleteValueW(hkey, PCWSTR(val_w.as_ptr()));
            let _ = RegCloseKey(hkey);
        }
    }
}

#[cfg(windows)]
fn write_launch_at_login_registry(enabled: bool) -> Result<bool, String> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    if !enabled {
        delete_registry_value_best_effort(STARTUP_RUN_SUBKEY, STARTUP_VALUE_NAME);
        delete_registry_value_best_effort(STARTUP_APPROVED_SUBKEY, STARTUP_VALUE_NAME);
        return Ok(false);
    }

    let exe = std::env::current_exe()
        .map_err(|e| format!("failed to resolve executable path for startup entry: {e}"))?;
    let cmd = format_startup_command(&exe);
    let cmd_w = to_wide_null(&cmd);
    let bytes: &[u8] =
        unsafe { std::slice::from_raw_parts(cmd_w.as_ptr().cast::<u8>(), cmd_w.len() * 2) };

    let subkey_w = to_wide_null(STARTUP_RUN_SUBKEY);
    let val_w = to_wide_null(STARTUP_VALUE_NAME);
    let mut hkey = HKEY::default();
    let create_res = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        )
    };
    if create_res.is_err() {
        return Err(format!(
            "failed to open Windows startup registry key: {:?}",
            create_res
        ));
    }

    let set_res = unsafe {
        RegSetValueExW(
            hkey,
            PCWSTR(val_w.as_ptr()),
            0,
            REG_SZ,
            Some(bytes),
        )
    };
    unsafe {
        let _ = RegCloseKey(hkey);
    }
    if set_res.is_err() {
        return Err(format!(
            "failed to write Windows startup registry value: {:?}",
            set_res
        ));
    }

    // Clear any stale disabled Task Manager approval entry so launch-at-login takes effect cleanly.
    delete_registry_value_best_effort(STARTUP_APPROVED_SUBKEY, STARTUP_VALUE_NAME);
    Ok(true)
}

#[tauri::command]
async fn get_launch_at_login() -> Result<bool, String> {
    #[cfg(windows)]
    {
        Ok(read_launch_at_login_registry())
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[tauri::command]
async fn set_launch_at_login(enabled: bool) -> Result<bool, String> {
    #[cfg(windows)]
    {
        write_launch_at_login_registry(enabled)
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("Launch at login is only implemented on Windows".into())
    }
}

#[tauri::command]
async fn get_service_status() -> Result<ServiceStateSummary, String> {
    Ok(query_service_status())
}

fn execute_service_action(action: &str) -> Result<(), String> {
    let daemon_exe = locate_daemon_executable()?;
    let cfg_path = default_config_path();

    if is_elevated() {
        match action {
            "install" => {
                install_service(&daemon_exe, &cfg_path, true).map_err(|e| e.to_string())?;
                start_service().map_err(|e| e.to_string())?;
            }
            "start" => start_service().map_err(|e| e.to_string())?,
            "stop" => stop_service().map_err(|e| e.to_string())?,
            "uninstall" => uninstall_service().map_err(|e| e.to_string())?,
            other => return Err(format!("unsupported service action '{other}'")),
        }
        Ok(())
    } else {
        #[cfg(windows)]
        {
            run_elevated_daemon_action(&daemon_exe, &cfg_path, action)
        }
        #[cfg(not(windows))]
        {
            Err("Windows Service management is only supported on Windows".into())
        }
    }
}

#[tauri::command]
async fn manage_windows_service(
    app: AppHandle,
    state: State<'_, Arc<AppBackend>>,
    action: String,
) -> Result<ServiceStateSummary, String> {
    execute_service_action(&action)?;

    match action.as_str() {
        "install" | "start" => {
            let pipe_ready = state.wait_for_service_pipe(Duration::from_secs(8)).await;
            if pipe_ready && !state.embedded.is_engine_running() {
                state.set_active_backend(BackendMode::Service);
            }
        }
        "stop" | "uninstall" => {
            tokio::time::sleep(Duration::from_millis(450)).await;
            let _ = state.embedded.reload_config_from_disk();
            state.set_active_backend(BackendMode::Embedded);
        }
        _ => {}
    }

    let _ = app.emit("bypath://refresh", ());
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
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn spawn_elevated_foreground(exe_path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_CANCELLED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb = to_wide_null("runas");
    let file = to_wide_null(&exe_path.display().to_string());
    let params = to_wide_null("--start-foreground --handoff");
    let work_dir = exe_path
        .parent()
        .map(|p| to_wide_null(&p.display().to_string()));

    let res = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            work_dir
                .as_ref()
                .map(|w| PCWSTR(w.as_ptr()))
                .unwrap_or(PCWSTR::null()),
            SW_SHOWNORMAL,
        )
    };

    let code = res.0 as isize;
    if code <= 32 {
        let last_err = unsafe { GetLastError() };
        if last_err == ERROR_CANCELLED || code == 5 {
            return Err("ELEVATION_CANCELLED".into());
        }
        return Err(format!(
            "failed to relaunch elevated foreground mode (code {code})"
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn run_elevated_daemon_action(
    daemon_exe: &Path,
    cfg_path: &Path,
    action: &str,
) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

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
        "try {{ $p = Start-Process -FilePath 'powershell.exe' -Verb RunAs -PassThru -Wait -WindowStyle Hidden -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-Command',\"{}\"; exit $p.ExitCode }} catch {{ exit 1223 }}",
        script.replace('"', "`\"")
    );

    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &ps_cmd])
        .creation_flags(0x0800_0000)
        .status()
        .map_err(|e| format!("failed to launch elevated service command: {e}"))?;

    if !status.success() {
        if status.code() == Some(1223) {
            return Err("SERVICE_ACTION_CANCELLED".into());
        }
        return Err("elevated service action failed".into());
    }
    Ok(())
}

fn spawn_event_forwarder(app: AppHandle, backend: Arc<AppBackend>) {
    tauri::async_runtime::spawn(async move {
        let mut embedded_rx = backend.embedded.telemetry().subscribe();
        let mut mode_rx = backend.subscribe_mode();
        loop {
            let current = backend.current_backend();
            let use_service = current == BackendMode::Service
                || (!backend.embedded.is_engine_running()
                    && backend.is_service_pipe_live().await);

            if use_service {
                if let Ok(mut pipe_rx) = backend.ipc.subscribe_events().await {
                    info!("GUI event stream attached to background service Named Pipe");
                    loop {
                        tokio::select! {
                            maybe_evt = pipe_rx.recv() => {
                                match maybe_evt {
                                    Some(evt) => {
                                        let _ = app.emit("bypath://event", &evt);
                                    }
                                    None => {
                                        warn!("background service event stream disconnected; falling back");
                                        break;
                                    }
                                }
                            }
                            changed = mode_rx.changed() => {
                                if changed.is_err() || *mode_rx.borrow() == BackendMode::Embedded {
                                    break;
                                }
                            }
                        }
                    }
                    continue;
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
                changed = mode_rx.changed() => {
                    if changed.is_err() {
                        break;
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

fn hide_tray_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(TRAY_WINDOW_LABEL) {
        let _ = win.hide();
    }
}

fn build_tray_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(
        app,
        TRAY_WINDOW_LABEL,
        WebviewUrl::App("index.html?view=tray".into()),
    )
    .title("Bypath Tray")
    .inner_size(TRAY_DEFAULT_WIDTH, TRAY_DEFAULT_HEIGHT)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(false)
    .build()
}

#[allow(clippy::too_many_arguments)]
fn compute_tray_popup_position(
    anchor_x: f64,
    anchor_y: f64,
    win_w: i32,
    win_h: i32,
    work_left: i32,
    work_top: i32,
    work_w: i32,
    work_h: i32,
    margin: i32,
) -> (i32, i32) {
    let work_right = work_left + work_w;
    let work_bottom = work_top + work_h;

    let min_x = work_left + margin;
    let max_x = (work_right - win_w - margin).max(min_x);
    let raw_x = (anchor_x - (win_w as f64 / 2.0)).round() as i32;
    let x = raw_x.clamp(min_x, max_x);

    let min_y = work_top + margin;
    let max_y = (work_bottom - win_h - margin).max(min_y);
    let anchor_yi = anchor_y.round() as i32;
    let midpoint_y = work_top + (work_h / 2);

    let raw_y = if anchor_yi >= midpoint_y {
        let base_bottom = anchor_yi.min(work_bottom);
        let above = base_bottom - win_h;
        if above >= min_y {
            above
        } else {
            anchor_yi
        }
    } else {
        let base_top = anchor_yi.max(work_top);
        if base_top + win_h <= work_bottom {
            base_top
        } else {
            base_top - win_h
        }
    };

    let y = raw_y.clamp(min_y, max_y);
    (x, y)
}

fn position_tray_window(app: &AppHandle, win: &WebviewWindow, anchor_x: f64, anchor_y: f64) {
    let size = win.outer_size().ok().filter(|s| s.width > 0 && s.height > 0);
    let scale = win.scale_factor().unwrap_or(1.0);
    let win_w = size
        .map(|s| s.width as i32)
        .unwrap_or_else(|| (TRAY_DEFAULT_WIDTH * scale).round() as i32);
    let win_h = size
        .map(|s| s.height as i32)
        .unwrap_or_else(|| (TRAY_DEFAULT_HEIGHT * scale).round() as i32);

    let anchor_xi = anchor_x.round() as i32;
    let anchor_yi = anchor_y.round() as i32;

    let target_monitor = app
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|m| {
                let pos = m.position();
                let sz = m.size();
                anchor_xi >= pos.x
                    && anchor_xi < pos.x + sz.width as i32
                    && anchor_yi >= pos.y
                    && anchor_yi < pos.y + sz.height as i32
            })
        })
        .or_else(|| win.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());

    let (x, y) = if let Some(monitor) = target_monitor {
        let work = monitor.work_area();
        let margin = (2.0 * monitor.scale_factor()).round() as i32;
        compute_tray_popup_position(
            anchor_x,
            anchor_y,
            win_w,
            win_h,
            work.position.x,
            work.position.y,
            work.size.width as i32,
            work.size.height as i32,
            margin,
        )
    } else {
        (
            (anchor_x - (win_w as f64 / 2.0)).round() as i32,
            (anchor_y - win_h as f64).round().max(0.0) as i32,
        )
    };

    let _ = win.set_position(PhysicalPosition::new(x, y));
}

fn toggle_tray_popover(app: &AppHandle, backend: &Arc<AppBackend>, anchor_x: f64, anchor_y: f64) {
    let win = match app.get_webview_window(TRAY_WINDOW_LABEL) {
        Some(w) => w,
        None => match build_tray_window(app) {
            Ok(w) => w,
            Err(err) => {
                warn!("failed to create custom tray menu window: {err}");
                return;
            }
        },
    };

    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }

    if let Ok(guard) = backend.tray_last_blur.lock() {
        if let Some(last_blur) = *guard {
            if last_blur.elapsed() < Duration::from_millis(280) {
                return;
            }
        }
    }

    if let Ok(mut anchor_guard) = backend.tray_last_anchor.lock() {
        *anchor_guard = Some((anchor_x, anchor_y));
    }

    position_tray_window(app, &win, anchor_x, anchor_y);
    let _ = app.emit("bypath://tray-opened", ());

    if let Ok(mut shown_guard) = backend.tray_last_shown.lock() {
        *shown_guard = Some(Instant::now());
    }
    let _ = win.show();
    let _ = win.set_focus();
}

fn setup_tray(app: &AppHandle, backend: Arc<AppBackend>) -> tauri::Result<()> {
    // Pre-warm the custom frameless tray popover window so right-clicking opens instantaneously.
    if let Err(err) = build_tray_window(app) {
        warn!("failed to pre-warm custom tray popover window: {err}");
    }

    let backend_for_tray = Arc::clone(&backend);
    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Bypath")
        .show_menu_on_left_click(false)
        .on_tray_icon_event(move |tray, event| match event {
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            }
            | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } => {
                hide_tray_window(tray.app_handle());
                reveal_main_window(tray.app_handle());
            }
            TrayIconEvent::Click {
                button: MouseButton::Right,
                button_state: MouseButtonState::Up,
                position,
                ..
            } => {
                toggle_tray_popover(
                    tray.app_handle(),
                    &backend_for_tray,
                    position.x,
                    position.y,
                );
            }
            _ => {}
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg(windows)]
struct SingleInstanceGuard(isize);

#[cfg(windows)]
impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        if self.0 != 0 && self.0 != -1 {
            unsafe {
                let handle = windows::Win32::Foundation::HANDLE(self.0 as *mut std::ffi::c_void);
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            }
        }
    }
}

#[cfg(windows)]
fn acquire_single_instance(wait_for_handoff: bool) -> Option<SingleInstanceGuard> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;

    let name_w = to_wide_null(SINGLE_INSTANCE_MUTEX);
    let max_attempts = if wait_for_handoff { 80 } else { 1 };

    for attempt in 0..max_attempts {
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name_w.as_ptr())) }.ok()?;
        let already_exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if !already_exists {
            return Some(SingleInstanceGuard(handle.0 as isize));
        }
        unsafe {
            let _ = CloseHandle(handle);
        }
        if attempt + 1 < max_attempts {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    notify_existing_instance();
    None
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
    run_with_args(&[]);
}

pub fn run_with_args(args: &[String]) {
    let launch_opts = LaunchOptions::parse(args);

    #[cfg(windows)]
    let single_instance_guard = if launch_opts.audit_instance {
        None
    } else {
        match acquire_single_instance(launch_opts.handoff) {
            Some(g) => Some(g),
            None => return,
        }
    };

    let initial_mode = if launch_opts.start_foreground {
        BackendMode::Embedded
    } else {
        BackendMode::Service
    };

    let backend = Arc::new(
        AppBackend::new(
            initial_mode,
            #[cfg(windows)]
            single_instance_guard,
        )
        .expect("failed to initialize AppBackend"),
    );

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
    let start_foreground = launch_opts.start_foreground;
    let start_minimized = launch_opts.minimized;
    let audit_instance = launch_opts.audit_instance;

    let app = tauri::Builder::default()
        .manage(Arc::clone(&backend))
        .setup(move |app| {
            let handle = app.handle().clone();
            if !audit_instance {
                setup_tray(&handle, Arc::clone(&backend))?;
                #[cfg(windows)]
                spawn_focus_listener(handle.clone());
            }
            if start_minimized && !audit_instance {
                if let Some(win) = handle.get_webview_window("main") {
                    if backend.minimize_to_tray.load(Ordering::Relaxed) {
                        let _ = win.hide();
                    } else {
                        let _ = win.minimize();
                    }
                }
            }
            spawn_event_forwarder(handle.clone(), Arc::clone(&backend));

            if start_foreground {
                let bg = Arc::clone(&backend);
                let h = handle.clone();
                tauri::async_runtime::spawn(async move {
                    if bg.is_service_pipe_live().await {
                        if let Ok(IpcResponse::Status { status }) =
                            bg.ipc.request(&IpcRequest::GetStatus).await
                        {
                            if status.engine_running {
                                let _ = bg.ipc.request(&IpcRequest::StopEngine).await;
                                tokio::time::sleep(Duration::from_millis(150)).await;
                            }
                        }
                    }
                    let _ = bg.embedded.reload_config_from_disk();
                    bg.set_active_backend(BackendMode::Embedded);
                    if let Err(err) = bg.embedded.start_engine().await {
                        warn!(error = %err, "failed to auto-start foreground mode after elevation");
                    }
                    let _ = h.emit("bypath://refresh", ());
                });
            }

            Ok(())
        })
        .on_window_event(move |window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                if window.label() == TRAY_WINDOW_LABEL {
                    if !backend_for_close.allow_exit.load(Ordering::SeqCst) {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                    return;
                }
                let should_hide = !audit_instance
                    && !backend_for_close.allow_exit.load(Ordering::SeqCst)
                    && backend_for_close.minimize_to_tray.load(Ordering::Relaxed);
                if should_hide {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
            WindowEvent::Focused(false) if window.label() == TRAY_WINDOW_LABEL => {
                let within_open_grace = backend_for_close
                    .tray_last_shown
                    .lock()
                    .ok()
                    .and_then(|g| *g)
                    .map(|t| t.elapsed() < Duration::from_millis(120))
                    .unwrap_or(false);
                if !within_open_grace && window.is_visible().unwrap_or(false) {
                    let _ = window.hide();
                    if let Ok(mut guard) = backend_for_close.tray_last_blur.lock() {
                        *guard = Some(Instant::now());
                    }
                }
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_config,
            save_config,
            start_engine,
            stop_engine,
            set_run_mode,
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
            open_main_window,
            hide_tray_menu,
            resize_tray_menu,
            sync_ui_prefs,
            quit_app,
            open_path_in_explorer,
            open_windows_security,
            check_for_updates,
            open_external_url,
            pick_directory,
            get_launch_at_login,
            set_launch_at_login,
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

#[cfg(test)]
mod tests {
    use super::{
        compute_tray_popup_position, format_startup_command, is_startup_approved_bytes,
        LaunchOptions,
    };
    use std::path::Path;

    #[test]
    fn parses_launch_options_flags() {
        let opts = LaunchOptions::parse(&[
            "--start-foreground".into(),
            "--handoff".into(),
            "--minimized".into(),
            "--unknown".into(),
        ]);
        assert!(opts.start_foreground);
        assert!(opts.handoff);
        assert!(opts.minimized);
        assert!(!opts.audit_instance);

        let audit = LaunchOptions::parse(&["--audit-instance".into()]);
        assert!(!audit.start_foreground);
        assert!(!audit.handoff);
        assert!(!audit.minimized);
        assert!(audit.audit_instance);
    }

    #[test]
    fn formats_quoted_startup_command_and_parses_approval_bytes() {
        let cmd = format_startup_command(Path::new(r"C:\Program Files\Bypath\bypath-gui.exe"));
        assert_eq!(cmd, r#""C:\Program Files\Bypath\bypath-gui.exe" --minimized"#);

        assert!(is_startup_approved_bytes(&[0x02, 0x00, 0x00, 0x00]));
        assert!(!is_startup_approved_bytes(&[0x03, 0x00, 0x00, 0x00]));
        assert!(is_startup_approved_bytes(&[]));
    }

    #[test]
    fn positions_above_bottom_taskbar_and_clamps_right_edge() {
        let (x, y) = compute_tray_popup_position(
            1880.0, 1056.0, 284, 388, 0, 0, 1920, 1032, 2,
        );
        assert_eq!(x, 1920 - 284 - 2);
        assert_eq!(y, 1032 - 388 - 2);
    }

    #[test]
    fn positions_below_top_taskbar() {
        let (x, y) = compute_tray_popup_position(
            1700.0, 20.0, 284, 388, 0, 48, 1920, 1032, 2,
        );
        assert_eq!(x, 1700 - 142);
        assert_eq!(y, 50);
    }

    #[test]
    fn positions_above_overflow_flyout_icon_within_work_area() {
        let (x, y) = compute_tray_popup_position(
            1740.0, 900.0, 284, 388, 0, 0, 1920, 1032, 2,
        );
        assert_eq!(x, 1740 - 142);
        assert_eq!(y, 900 - 388);
    }

    #[test]
    fn clamps_left_edge_on_secondary_monitor_with_negative_origin() {
        let (x, y) = compute_tray_popup_position(
            -1900.0, 1050.0, 284, 388, -1920, 0, 1920, 1032, 4,
        );
        assert_eq!(x, -1916);
        assert_eq!(y, 1032 - 388 - 4);
    }
}
