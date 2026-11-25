use crate::service::query_service_status;
use anyhow::{anyhow, Result};
use bypath_core::capture::{CaptureEngine, FiveTuple, TransportProtocol};
use bypath_core::config::{
    list_builtin_presets, resolve_windivert_dir, Config, ProxyConfig,
};
use bypath_core::engine::{EngineHandle, ProxyEngine};
use bypath_core::process::{ProcessTracker, SnapshotProcessTracker};
use bypath_core::rules::{RouteDecision, RuleEngine};
use bypath_core::telemetry::{EngineEvent, TelemetryHub};
use bypath_core::upstream::probe::probe_proxy;
use bypath_ipc::{
    BackendMode, ConfigSnapshot, DaemonStatus, DoctorReport, IpcHandler, IpcRequest, IpcResponse,
    NamedProxyProbe, ProcessEntry,
};
use bypath_windivert::{is_elevated, probe_windivert, WinDivertCaptureEngine};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::{broadcast, oneshot, Mutex};
use tracing::{info, warn};

struct RunningEngine {
    handle: EngineHandle,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join_handle: tokio::task::JoinHandle<()>,
}

/// Shared daemon/embedded state machine handling configuration, live engine lifecycle,
/// telemetry streaming, diagnostics, and IPC requests.
pub struct DaemonController {
    backend_mode: BackendMode,
    config_path: PathBuf,
    config: RwLock<Config>,
    tracker: Arc<SnapshotProcessTracker>,
    telemetry: Arc<TelemetryHub>,
    active: Mutex<Option<RunningEngine>>,
    last_error: Arc<RwLock<Option<String>>>,
}

impl DaemonController {
    pub fn new(config_path: PathBuf, backend_mode: BackendMode) -> Result<Arc<Self>> {
        let config = if config_path.exists() {
            match Config::from_file(&config_path) {
                Ok(cfg) => cfg,
                Err(err) => {
                    warn!(
                        path = %config_path.display(),
                        error = %err,
                        "failed to load existing config; falling back to starter default"
                    );
                    Config::starter_default(
                        resolve_windivert_dir(None).display().to_string(),
                    )
                }
            }
        } else {
            Config::starter_default(resolve_windivert_dir(None).display().to_string())
        };

        Ok(Arc::new(Self {
            backend_mode,
            config_path,
            config: RwLock::new(config),
            tracker: Arc::new(SnapshotProcessTracker::default()),
            telemetry: TelemetryHub::new(),
            active: Mutex::new(None),
            last_error: Arc::new(RwLock::new(None)),
        }))
    }

    pub fn telemetry(&self) -> Arc<TelemetryHub> {
        Arc::clone(&self.telemetry)
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    pub fn current_config(&self) -> Config {
        self.config.read().expect("config lock poisoned").clone()
    }

    pub async fn start_engine(&self) -> Result<()> {
        let mut guard = self.active.lock().await;
        if let Some(ref running) = *guard {
            if !running.join_handle.is_finished() {
                return Ok(());
            }
        }
        *guard = None;

        let cfg = self.current_config();
        cfg.validate()?;

        let wd_dir = resolve_windivert_dir(Some(&cfg.general.windivert_dir));
        let tracker_trait: Arc<dyn ProcessTracker> = self.tracker.clone();
        let capture: Arc<dyn CaptureEngine> = Arc::new(
            WinDivertCaptureEngine::with_process_tracker(Some(&wd_dir), Some(tracker_trait.clone()))
                .inspect_err(|err| {
                    self.set_last_error(Some(err.to_string()));
                })?,
        );

        // Preflight WinDivert open check so non-elevated callers get an immediate, crisp error.
        let probe = probe_windivert(Some(&wd_dir));
        if !probe.driver_opened {
            let msg = probe.error.unwrap_or_else(|| {
                "WinDivert kernel driver could not be opened (Administrator privileges required)"
                    .to_string()
            });
            self.set_last_error(Some(msg.clone()));
            return Err(anyhow!(msg));
        }

        let engine = ProxyEngine::with_telemetry(
            cfg,
            capture,
            tracker_trait,
            Arc::clone(&self.telemetry),
        )?;
        let handle = engine.handle();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let last_err_ref = Arc::clone(&self.last_error);
        let telemetry = Arc::clone(&self.telemetry);

        let join_handle = tokio::spawn(async move {
            if let Err(err) = engine.run(shutdown_rx).await {
                warn!(error = %err, "proxy engine exited with error");
                if let Ok(mut e) = last_err_ref.write() {
                    *e = Some(err.to_string());
                }
                telemetry.set_running(false, Some(err.to_string()));
            }
        });

        // Wait briefly for capture.start() to mark running or exit immediately on error.
        for _ in 0..20 {
            if self.telemetry.is_running() || join_handle.is_finished() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        if join_handle.is_finished() && !self.telemetry.is_running() {
            let msg = "proxy engine terminated immediately during startup".to_string();
            self.set_last_error(Some(msg.clone()));
            return Err(anyhow!(msg));
        }

        self.set_last_error(None);
        *guard = Some(RunningEngine {
            handle,
            shutdown_tx: Some(shutdown_tx),
            join_handle,
        });
        info!("daemon started proxy engine");
        Ok(())
    }

    pub async fn stop_engine(&self) -> Result<()> {
        let mut guard = self.active.lock().await;
        if let Some(mut running) = guard.take() {
            if let Some(tx) = running.shutdown_tx.take() {
                let _ = tx.send(());
            }
            let _ = tokio::time::timeout(Duration::from_secs(4), running.join_handle).await;
            info!("daemon stopped proxy engine");
        }
        self.telemetry.set_running(false, None);
        Ok(())
    }

    pub async fn save_config(&self, new_cfg: Config, reload: bool) -> Result<()> {
        new_cfg.validate()?;
        new_cfg.save_to_file(&self.config_path)?;
        {
            let mut guard = self.config.write().expect("config lock poisoned");
            *guard = new_cfg.clone();
        }
        if reload {
            let guard = self.active.lock().await;
            if let Some(ref running) = *guard {
                if !running.join_handle.is_finished() {
                    running.handle.reload_config(new_cfg)?;
                }
            }
        }
        Ok(())
    }

    pub async fn status(&self) -> DaemonStatus {
        let cfg = self.current_config();
        let wd_dir = resolve_windivert_dir(Some(&cfg.general.windivert_dir));
        let wd = probe_windivert(Some(&wd_dir));
        let svc = query_service_status();
        let engine_running = self.telemetry.is_running();
        let active_rules_count = cfg.rules.iter().filter(|r| r.enabled).count();

        DaemonStatus {
            backend_mode: self.backend_mode,
            daemon_pid: std::process::id(),
            elevated: is_elevated(),
            service_installed: svc.installed,
            service_running: svc.running,
            engine_running,
            windivert_ready: wd.dll_loaded && wd.driver_opened,
            windivert_dll_path: wd.dll_path,
            windivert_version: wd.driver_version.map(|(maj, min)| format!("v{maj}.{min}")),
            windivert_error: wd.error,
            config_path: self.config_path.display().to_string(),
            active_rules_count,
            total_rules_count: cfg.rules.len(),
            proxies_count: cfg.proxies.len(),
            last_error: self.last_error.read().ok().and_then(|e| e.clone()),
            stats: self.telemetry.stats_snapshot(),
        }
    }

    pub fn list_processes(&self, filter: Option<&str>) -> Vec<ProcessEntry> {
        let cfg = self.current_config();
        let rule_engine = RuleEngine::from_config(&cfg).ok();
        let probe_tuple = FiveTuple {
            protocol: TransportProtocol::Tcp,
            src_ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)),
            src_port: 55000,
            dst_ip: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            dst_port: 443,
        };
        let needle = filter
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty());

        let mut out = Vec::new();
        for p in self.tracker.list_processes() {
            if let Some(ref n) = needle {
                let matches_name = p.name.to_ascii_lowercase().contains(n);
                let matches_path = p
                    .exe_path
                    .as_deref()
                    .map(|path| path.to_ascii_lowercase().contains(n))
                    .unwrap_or(false);
                if !matches_name && !matches_path {
                    continue;
                }
            }

            let matched_rule = rule_engine.as_ref().and_then(|re| {
                match re.evaluate(Some(p.pid), &probe_tuple, self.tracker.as_ref()) {
                    RouteDecision::Proxy { rule_name, .. }
                    | RouteDecision::Block { rule_name }
                    | RouteDecision::InspectDirect { rule_name } => Some(rule_name),
                    RouteDecision::Direct { .. } => None,
                }
            });

            out.push(ProcessEntry {
                pid: p.pid,
                parent_pid: p.parent_pid,
                name: p.name,
                exe_path: p.exe_path,
                matched_rule,
            });
        }
        out
    }

    pub async fn run_doctor(&self) -> DoctorReport {
        let cfg = self.current_config();
        let wd_dir = resolve_windivert_dir(Some(&cfg.general.windivert_dir));
        let wd = probe_windivert(Some(&wd_dir));

        let mut proxy_names: Vec<String> = cfg.proxies.keys().cloned().collect();
        proxy_names.sort();

        let mut probes = Vec::with_capacity(proxy_names.len());
        for name in proxy_names {
            if let Some(pcfg) = cfg.proxies.get(&name) {
                probes.push(probe_named_proxy(name, pcfg).await);
            }
        }

        DoctorReport {
            elevated: is_elevated(),
            windivert_dll_loaded: wd.dll_loaded,
            windivert_dll_path: wd.dll_path,
            windivert_driver_opened: wd.driver_opened,
            windivert_version: wd.driver_version.map(|(maj, min)| format!("v{maj}.{min}")),
            windivert_error: wd.error,
            proxies: probes,
        }
    }

    fn set_last_error(&self, err: Option<String>) {
        if let Ok(mut guard) = self.last_error.write() {
            *guard = err;
        }
    }
}

pub async fn probe_named_proxy(name: String, proxy: &ProxyConfig) -> NamedProxyProbe {
    let report = probe_proxy(proxy, Duration::from_secs(3)).await;
    let local_listener_pid = proxy
        .parse_socket_addr()
        .ok()
        .filter(|sa| sa.ip().is_loopback())
        .and_then(|sa| bypath_windivert::iphelper::find_tcp_listener_pid(sa.port()));
    NamedProxyProbe {
        name,
        report,
        local_listener_pid,
    }
}

impl IpcHandler for DaemonController {
    fn handle(
        &self,
        request: IpcRequest,
    ) -> Pin<Box<dyn Future<Output = IpcResponse> + Send + '_>> {
        Box::pin(async move {
            match request {
                IpcRequest::Ping => IpcResponse::Pong {
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
                IpcRequest::GetStatus => IpcResponse::Status {
                    status: self.status().await,
                },
                IpcRequest::GetConfig => IpcResponse::Config {
                    snapshot: ConfigSnapshot {
                        path: self.config_path.display().to_string(),
                        config: self.current_config(),
                    },
                },
                IpcRequest::SaveConfig { config, reload } => {
                    match self.save_config(config, reload).await {
                        Ok(()) => IpcResponse::Ok {
                            message: "configuration saved".into(),
                        },
                        Err(err) => IpcResponse::Error {
                            message: err.to_string(),
                        },
                    }
                }
                IpcRequest::StartEngine => match self.start_engine().await {
                    Ok(()) => IpcResponse::Ok {
                        message: "engine started".into(),
                    },
                    Err(err) => IpcResponse::Error {
                        message: err.to_string(),
                    },
                },
                IpcRequest::StopEngine => match self.stop_engine().await {
                    Ok(()) => IpcResponse::Ok {
                        message: "engine stopped".into(),
                    },
                    Err(err) => IpcResponse::Error {
                        message: err.to_string(),
                    },
                },
                IpcRequest::GetConnections => IpcResponse::Connections {
                    connections: self.telemetry.list_connections(),
                },
                IpcRequest::ClearConnections => {
                    self.telemetry.clear_closed_connections();
                    IpcResponse::Ok {
                        message: "closed connections cleared".into(),
                    }
                }
                IpcRequest::ListProcesses { filter } => IpcResponse::Processes {
                    processes: self.list_processes(filter.as_deref()),
                },
                IpcRequest::RunDoctor => IpcResponse::Doctor {
                    report: self.run_doctor().await,
                },
                IpcRequest::ProbeProxy { proxy } => IpcResponse::ProxyProbe {
                    probe: probe_named_proxy("probe".into(), &proxy).await,
                },
                IpcRequest::ListPresets => IpcResponse::Presets {
                    presets: list_builtin_presets(),
                },
                IpcRequest::SubscribeEvents => IpcResponse::Subscribed,
            }
        })
    }

    fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.telemetry.subscribe()
    }
}
