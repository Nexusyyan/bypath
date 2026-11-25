use anyhow::{anyhow, bail, Result};
use bypath_core::config::{default_config_path, Config, ProxyConfig, ProxyKind, RuleAction, RuleConfig};
use bypath_core::rules::{RuleTestQuery, RuleTestReport};
use bypath_core::telemetry::{ConnectionRecord, LogEntry};
use bypath_core::update::{check_for_updates as core_check_updates, UpdateCheckReport};
use bypath_core::upstream::system_proxy::SystemProxyInfo;
use bypath_daemon::DaemonController;
use bypath_ipc::{
    BackendMode, ConfigSnapshot, DaemonStatus, DoctorReport, IpcClient, IpcHandler, IpcRequest,
    IpcResponse, NamedProxyProbe, ProcessEntry,
};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Unified control client shared by `bypath ctl` and `bypath mcp`.
/// Uses the running Windows Service / Named Pipe daemon when online, and transparently
/// falls back to an embedded `DaemonController` so offline config editing, rule testing,
/// and system-proxy inspection work seamlessly even when the service is stopped.
pub struct AgentClient {
    ipc: IpcClient,
    embedded: Arc<DaemonController>,
}

impl AgentClient {
    pub fn new(pipe: impl Into<String>, config_path: Option<PathBuf>) -> Result<Self> {
        let cfg_path = config_path.unwrap_or_else(default_config_path);
        let embedded = DaemonController::new(cfg_path, BackendMode::Embedded)?;
        Ok(Self {
            ipc: IpcClient::new(pipe),
            embedded,
        })
    }

    pub async fn dispatch(&self, req: IpcRequest) -> Result<IpcResponse> {
        if let Ok(resp) = self.ipc.request(&req).await {
            return Ok(resp);
        }
        Ok(self.embedded.handle(req).await)
    }

    pub async fn get_status(&self) -> Result<DaemonStatus> {
        match self.dispatch(IpcRequest::GetStatus).await? {
            IpcResponse::Status { status } => Ok(status),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected status response: {other:?}"),
        }
    }

    pub async fn get_config(&self) -> Result<ConfigSnapshot> {
        match self.dispatch(IpcRequest::GetConfig).await? {
            IpcResponse::Config { snapshot } => Ok(snapshot),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected config response: {other:?}"),
        }
    }

    pub async fn save_config(&self, config: Config, reload: bool) -> Result<String> {
        match self
            .dispatch(IpcRequest::SaveConfig { config, reload })
            .await?
        {
            IpcResponse::Ok { message } => Ok(message),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected save_config response: {other:?}"),
        }
    }

    pub async fn start_engine(&self) -> Result<String> {
        match self.dispatch(IpcRequest::StartEngine).await? {
            IpcResponse::Ok { message } => Ok(message),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected start_engine response: {other:?}"),
        }
    }

    pub async fn stop_engine(&self) -> Result<String> {
        match self.dispatch(IpcRequest::StopEngine).await? {
            IpcResponse::Ok { message } => Ok(message),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected stop_engine response: {other:?}"),
        }
    }

    pub async fn get_connections(&self) -> Result<Vec<ConnectionRecord>> {
        match self.dispatch(IpcRequest::GetConnections).await? {
            IpcResponse::Connections { connections } => Ok(connections),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected connections response: {other:?}"),
        }
    }

    pub async fn clear_connections(&self) -> Result<String> {
        match self.dispatch(IpcRequest::ClearConnections).await? {
            IpcResponse::Ok { message } => Ok(message),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected clear_connections response: {other:?}"),
        }
    }

    pub async fn list_processes(&self, filter: Option<String>) -> Result<Vec<ProcessEntry>> {
        match self.dispatch(IpcRequest::ListProcesses { filter }).await? {
            IpcResponse::Processes { processes } => Ok(processes),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected processes response: {other:?}"),
        }
    }

    pub async fn run_doctor(&self) -> Result<DoctorReport> {
        match self.dispatch(IpcRequest::RunDoctor).await? {
            IpcResponse::Doctor { report } => Ok(report),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected doctor response: {other:?}"),
        }
    }

    pub async fn probe_proxy(&self, proxy: ProxyConfig) -> Result<NamedProxyProbe> {
        match self.dispatch(IpcRequest::ProbeProxy { proxy }).await? {
            IpcResponse::ProxyProbe { probe } => Ok(probe),
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected probe response: {other:?}"),
        }
    }

    pub async fn get_system_proxy(&self) -> Result<SystemProxyInfo> {
        // Always read Windows HKCU Internet Settings & env vars in the interactive user's
        // process rather than delegating to LocalSystem BypathService.
        Ok(bypath_core::upstream::system_proxy::read_system_proxy())
    }

    pub async fn test_rule(&self, query: RuleTestQuery) -> Result<RuleTestReport> {
        match self.dispatch(IpcRequest::TestRule { query: query.clone() }).await? {
            IpcResponse::RuleTest { report } => Ok(report),
            IpcResponse::Error { message } if message.contains("unknown variant") => {
                // Backward-compatible fallback when connected to an older BypathService.
                let snap = self.get_config().await?;
                let engine = bypath_core::rules::RuleEngine::from_config(&snap.config)?;
                Ok(engine.test_query(&query))
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected rule_test response: {other:?}"),
        }
    }

    pub async fn get_logs(
        &self,
        min_level: Option<String>,
        limit: Option<usize>,
    ) -> Result<Vec<LogEntry>> {
        match self
            .dispatch(IpcRequest::GetLogs {
                min_level: min_level.clone(),
                limit,
            })
            .await?
        {
            IpcResponse::Logs { entries } => Ok(entries),
            IpcResponse::Error { message } if message.contains("unknown variant") => {
                Ok(self.embedded.list_logs(min_level.as_deref(), limit))
            }
            IpcResponse::Error { message } => bail!("{message}"),
            other => bail!("unexpected logs response: {other:?}"),
        }
    }

    /// Creates or updates a proxy profile, optionally renaming it and cascading rule references.
    #[allow(clippy::too_many_arguments)]
    pub async fn upsert_proxy(
        &self,
        name: &str,
        kind: ProxyKind,
        address: String,
        username: Option<String>,
        password: Option<String>,
        udp: bool,
        rename_to: Option<String>,
    ) -> Result<ConfigSnapshot> {
        let mut snap = self.get_config().await?;
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            bail!("proxy name cannot be empty");
        }
        let target_name = rename_to
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(trimmed_name)
            .to_string();

        if target_name != trimmed_name {
            snap.config.proxies.remove(trimmed_name);
            for rule in &mut snap.config.rules {
                if rule.proxy.as_deref() == Some(trimmed_name) {
                    rule.proxy = Some(target_name.clone());
                }
            }
        }

        snap.config.proxies.insert(
            target_name,
            ProxyConfig {
                kind,
                address: address.trim().to_string(),
                username: username.filter(|s| !s.trim().is_empty()),
                password: password.filter(|s| !s.trim().is_empty()),
                udp: kind == ProxyKind::Socks5 && udp,
            },
        );

        self.save_config(snap.config.clone(), true).await?;
        Ok(snap)
    }

    pub async fn remove_proxy(&self, name: &str) -> Result<ConfigSnapshot> {
        let mut snap = self.get_config().await?;
        if !snap.config.proxies.contains_key(name) {
            bail!("proxy '{name}' not found");
        }
        let in_use = snap
            .config
            .rules
            .iter()
            .any(|r| r.enabled && r.action == RuleAction::Proxy && r.proxy.as_deref() == Some(name));
        if in_use {
            bail!("proxy '{name}' is referenced by an enabled rule; update or disable that rule first");
        }
        snap.config.proxies.remove(name);
        self.save_config(snap.config.clone(), true).await?;
        Ok(snap)
    }

    /// Inserts or replaces a rule. When `index` is provided, updates that rule in place;
    /// otherwise updates an existing rule with the exact same name or appends a new rule.
    pub async fn upsert_rule(
        &self,
        index: Option<usize>,
        rule: RuleConfig,
        prepend: bool,
    ) -> Result<(usize, ConfigSnapshot)> {
        let mut snap = self.get_config().await?;
        let final_idx = if let Some(idx) = index {
            if idx >= snap.config.rules.len() {
                bail!("rule index {idx} out of range (total {})", snap.config.rules.len());
            }
            snap.config.rules[idx] = rule;
            idx
        } else if let Some(existing_idx) = snap
            .config
            .rules
            .iter()
            .position(|r| r.name.eq_ignore_ascii_case(&rule.name))
        {
            snap.config.rules[existing_idx] = rule;
            existing_idx
        } else if prepend {
            snap.config.rules.insert(0, rule);
            0
        } else {
            snap.config.rules.push(rule);
            snap.config.rules.len() - 1
        };

        self.save_config(snap.config.clone(), true).await?;
        Ok((final_idx, snap))
    }

    pub async fn move_rule(&self, from: usize, to: usize) -> Result<ConfigSnapshot> {
        let mut snap = self.get_config().await?;
        let len = snap.config.rules.len();
        if from >= len || to >= len {
            bail!("rule move indices ({from} -> {to}) out of range (total {len})");
        }
        let item = snap.config.rules.remove(from);
        snap.config.rules.insert(to, item);
        self.save_config(snap.config.clone(), true).await?;
        Ok(snap)
    }

    pub async fn toggle_rule(
        &self,
        selector: &str,
        enabled_override: Option<bool>,
    ) -> Result<(usize, bool, ConfigSnapshot)> {
        let mut snap = self.get_config().await?;
        let idx = resolve_rule_index(&snap.config.rules, selector)?;
        let next = enabled_override.unwrap_or(!snap.config.rules[idx].enabled);
        snap.config.rules[idx].enabled = next;
        self.save_config(snap.config.clone(), true).await?;
        Ok((idx, next, snap))
    }

    pub async fn delete_rule(&self, selector: &str) -> Result<(RuleConfig, ConfigSnapshot)> {
        let mut snap = self.get_config().await?;
        let idx = resolve_rule_index(&snap.config.rules, selector)?;
        let removed = snap.config.rules.remove(idx);
        self.save_config(snap.config.clone(), true).await?;
        Ok((removed, snap))
    }

    pub async fn check_for_updates(&self, proxy_uri: Option<&str>) -> Result<UpdateCheckReport> {
        let cfg = self.get_config().await.ok().map(|snap| snap.config);
        let explicit = match proxy_uri {
            Some(uri) if !uri.trim().is_empty() => Some(ProxyConfig::from_uri(uri.trim(), false)?),
            _ => None,
        };
        core_check_updates(cfg.as_ref(), explicit.as_ref(), Duration::from_secs(12)).await
    }
}

pub fn resolve_rule_index(rules: &[RuleConfig], selector: &str) -> Result<usize> {
    let trimmed = selector.trim();
    if let Ok(idx) = trimmed.parse::<usize>() {
        if idx < rules.len() {
            return Ok(idx);
        }
    }
    rules
        .iter()
        .position(|r| r.name.eq_ignore_ascii_case(trimmed))
        .ok_or_else(|| anyhow!("rule '{trimmed}' not found by index or name"))
}

pub fn parse_proxy_kind(raw: &str) -> Result<ProxyKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "socks5" | "socks5h" | "socks" => Ok(ProxyKind::Socks5),
        "http" | "https" => Ok(ProxyKind::Http),
        other => bail!("unsupported proxy type '{other}': expected socks5 or http"),
    }
}

pub fn parse_rule_action(raw: &str) -> Result<RuleAction> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "proxy" => Ok(RuleAction::Proxy),
        "direct" => Ok(RuleAction::Direct),
        "block" => Ok(RuleAction::Block),
        other => bail!("unsupported rule action '{other}': expected proxy, direct, or block"),
    }
}

/// Machine-readable capability manifest for AI Agents and automation tooling.
pub fn capability_schema() -> Value {
    json!({
        "name": "bypath",
        "version": env!("CARGO_PKG_VERSION"),
        "pipe": bypath_ipc::DEFAULT_PIPE_NAME,
        "interfaces": {
            "cli_json": "bypath ctl --json <subcommand>",
            "mcp_stdio": "bypath mcp"
        },
        "evaluation_semantics": {
            "order": "first_match_wins_top_to_bottom",
            "phase1_l4": "Evaluates process_names, path_prefixes, path_contains (with optional include_children tree inheritance), dst_cidrs, dst_ports, and protocols on TCP SYN / UDP flows.",
            "phase2_l7": "After TLS SNI / HTTP Host sniffing, domain-bearing rules override Phase 1 with Block, Direct, or Proxy."
        },
        "cli_commands": [
            "ctl --json status",
            "ctl --json start",
            "ctl --json stop",
            "ctl --json doctor",
            "ctl --json system-proxy [--apply]",
            "ctl --json config get|export <file>|import <file>",
            "ctl --json proxy list|set <name>|remove <name>|probe <name>",
            "ctl --json rule list|add|update <sel>|move <from> <to>|toggle <sel>|remove <sel>|test",
            "ctl --json connections [--process <name>] [--state <state>] [--limit <n>]",
            "ctl --json logs [--level info|warn|error] [--limit <n>]",
            "ctl --json update [--proxy <uri>]",
            "ctl schema",
            "mcp"
        ],
        "mcp_tools": [
            "bypath_status",
            "bypath_toggle_engine",
            "bypath_get_system_proxy",
            "bypath_get_config",
            "bypath_upsert_proxy",
            "bypath_upsert_rule",
            "bypath_reorder_rules",
            "bypath_delete_rule",
            "bypath_test_rule",
            "bypath_list_connections",
            "bypath_list_processes",
            "bypath_get_logs",
            "bypath_check_updates"
        ]
    })
}
