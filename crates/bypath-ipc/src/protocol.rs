use bypath_core::config::{Config, PresetInfo, ProxyConfig};
use bypath_core::rules::{RuleTestQuery, RuleTestReport};
use bypath_core::telemetry::{ConnectionRecord, EngineStats, LogEntry};
use bypath_core::upstream::probe::ProxyProbeReport;
use bypath_core::upstream::system_proxy::SystemProxyInfo;
use serde::{Deserialize, Serialize};

/// Default Windows Named Pipe endpoint shared between the background service, CLI, and GUI.
pub const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\bypath-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendMode {
    Service,
    Embedded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub backend_mode: BackendMode,
    pub daemon_pid: u32,
    pub elevated: bool,
    pub service_installed: bool,
    pub service_running: bool,
    pub engine_running: bool,
    pub windivert_ready: bool,
    pub windivert_dll_path: Option<String>,
    pub windivert_version: Option<String>,
    pub windivert_error: Option<String>,
    #[serde(default)]
    pub windivert_error_kind: Option<String>,
    #[serde(default)]
    pub portable_mode: bool,
    pub config_path: String,
    pub active_rules_count: usize,
    pub total_rules_count: usize,
    pub proxies_count: usize,
    pub last_error: Option<String>,
    pub stats: EngineStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSnapshot {
    pub path: String,
    pub config: Config,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
    pub matched_rule: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedProxyProbe {
    pub name: String,
    pub report: ProxyProbeReport,
    pub local_listener_pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub elevated: bool,
    pub windivert_dll_loaded: bool,
    pub windivert_dll_path: Option<String>,
    pub windivert_driver_opened: bool,
    pub windivert_version: Option<String>,
    pub windivert_error: Option<String>,
    #[serde(default)]
    pub windivert_error_kind: Option<String>,
    pub proxies: Vec<NamedProxyProbe>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum IpcRequest {
    Ping,
    GetStatus,
    GetConfig,
    SaveConfig {
        config: Config,
        #[serde(default = "default_true")]
        reload: bool,
    },
    StartEngine,
    StopEngine,
    GetConnections,
    ClearConnections,
    ListProcesses {
        filter: Option<String>,
    },
    RunDoctor,
    ProbeProxy {
        proxy: ProxyConfig,
    },
    ListPresets,
    GetSystemProxy,
    TestRule {
        query: RuleTestQuery,
    },
    GetLogs {
        #[serde(default)]
        min_level: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    SubscribeEvents,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    Pong { version: String },
    Status { status: DaemonStatus },
    Config { snapshot: ConfigSnapshot },
    Ok { message: String },
    Connections { connections: Vec<ConnectionRecord> },
    Processes { processes: Vec<ProcessEntry> },
    Doctor { report: DoctorReport },
    ProxyProbe { probe: NamedProxyProbe },
    Presets { presets: Vec<PresetInfo> },
    SystemProxy { info: SystemProxyInfo },
    RuleTest { report: RuleTestReport },
    Logs { entries: Vec<LogEntry> },
    Subscribed,
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_request_response_json_roundtrip() {
        let req = IpcRequest::ListProcesses {
            filter: Some("antigravity".into()),
        };
        let encoded = serde_json::to_string(&req).unwrap();
        let decoded: IpcRequest = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded,
            IpcRequest::ListProcesses { filter: Some(ref f) } if f == "antigravity"
        ));

        let test_req = IpcRequest::TestRule {
            query: RuleTestQuery {
                process_name: Some("Antigravity.exe".into()),
                exe_path: None,
                host: Some("play.googleapis.com".into()),
                dst_ip: None,
                dst_port: 443,
                protocol: "tcp".into(),
            },
        };
        let enc_test = serde_json::to_string(&test_req).unwrap();
        let dec_test: IpcRequest = serde_json::from_str(&enc_test).unwrap();
        assert!(matches!(dec_test, IpcRequest::TestRule { .. }));

        let resp = IpcResponse::Pong {
            version: "0.4.0".into(),
        };
        let encoded_resp = serde_json::to_string(&resp).unwrap();
        let decoded_resp: IpcResponse = serde_json::from_str(&encoded_resp).unwrap();
        assert!(matches!(
            decoded_resp,
            IpcResponse::Pong { ref version } if version == "0.4.0"
        ));
    }
}
