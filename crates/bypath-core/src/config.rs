use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub proxies: HashMap<String, ProxyConfig>,
    #[serde(default)]
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default = "default_true")]
    pub enable_sniffing: bool,
    #[serde(default = "default_true")]
    pub block_quic_when_no_udp: bool,
    #[serde(default = "default_true")]
    pub bypass_lan: bool,
    #[serde(default = "default_windivert_dir")]
    pub windivert_dir: String,
    /// When enabled, closing the desktop window hides it to the system tray instead of exiting.
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    /// When enabled, BypathService automatically starts packet interception on service startup.
    #[serde(default)]
    pub auto_start_engine: bool,
    /// UI theme preference: `"system"`, `"light"`, or `"dark"`.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// When enabled, the desktop app silently checks GitHub Releases on startup (throttled).
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            enable_sniffing: true,
            block_quic_when_no_udp: true,
            bypass_lan: true,
            windivert_dir: default_windivert_dir(),
            minimize_to_tray: true,
            auto_start_engine: false,
            theme: default_theme(),
            auto_check_updates: true,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_true() -> bool {
    true
}

fn default_windivert_dir() -> String {
    "runtime/windivert".to_string()
}

fn default_theme() -> String {
    "system".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProxyKind {
    Socks5,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    #[serde(rename = "type")]
    pub kind: ProxyKind,
    pub address: String,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub udp: bool,
}

impl ProxyConfig {
    /// Parses a proxy URI like `socks5://[user:pass@]127.0.0.1:7890` or `http://127.0.0.1:7890`.
    pub fn from_uri(uri: &str, udp: bool) -> Result<Self> {
        let (scheme, rest) = uri
            .split_once("://")
            .ok_or_else(|| anyhow!("invalid proxy URI '{uri}': expected scheme://host:port"))?;

        let kind = match scheme.to_ascii_lowercase().as_str() {
            "socks5" | "socks5h" => ProxyKind::Socks5,
            "http" => ProxyKind::Http,
            other => bail!("unsupported proxy scheme '{other}': use socks5:// or http://"),
        };

        let (creds, addr) = match rest.rsplit_once('@') {
            Some((c, a)) => (Some(c), a),
            None => (None, rest),
        };

        let (username, password) = match creds {
            Some(c) => match c.split_once(':') {
                Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
                None => (Some(c.to_string()), None),
            },
            None => (None, None),
        };

        // Strip trailing slash if supplied
        let addr = addr.trim_end_matches('/');
        if !addr.contains(':') {
            bail!("proxy URI '{uri}' is missing a port number");
        }

        Ok(Self {
            kind,
            address: addr.to_string(),
            username,
            password,
            udp: kind == ProxyKind::Socks5 && udp,
        })
    }

    pub fn parse_socket_addr(&self) -> Result<SocketAddr> {
        self.address
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid proxy socket address '{}'", self.address))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Proxy,
    Direct,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub process_names: Vec<String>,
    #[serde(default)]
    pub path_prefixes: Vec<String>,
    #[serde(default)]
    pub path_contains: Vec<String>,
    #[serde(default = "default_true")]
    pub include_children: bool,
    #[serde(default)]
    pub dst_cidrs: Vec<String>,
    #[serde(default)]
    pub dst_ports: Vec<u16>,
    #[serde(default)]
    pub protocols: Vec<String>,
    /// Optional domain / SNI / HTTP Host patterns (exact, `*.suffix`, `.suffix`, or glob `*`).
    #[serde(default)]
    pub domains: Vec<String>,
    pub action: RuleAction,
    #[serde(default)]
    pub proxy: Option<String>,
}

impl Config {
    pub fn from_file(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        let cfg: Config = toml::from_str(&raw)
            .with_context(|| format!("failed to parse TOML config {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validates and atomically writes this configuration to disk as pretty TOML.
    pub fn save_to_file(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create config directory {}", parent.display())
                })?;
            }
        }
        let body = toml::to_string_pretty(self).context("failed to serialize config to TOML")?;
        let tmp_path = path.with_extension("toml.tmp");
        std::fs::write(&tmp_path, body)
            .with_context(|| format!("failed to write temporary config {}", tmp_path.display()))?;
        if let Err(rename_err) = std::fs::rename(&tmp_path, path) {
            // Fallback for Windows when target file already exists in some filesystems.
            let _ = std::fs::remove_file(path);
            std::fs::rename(&tmp_path, path).with_context(|| {
                format!(
                    "failed to replace config {} (initial rename error: {rename_err})",
                    path.display()
                )
            })?;
        }
        Ok(())
    }

    /// Creates a sensible starter configuration ready for first-run GUI / daemon startup.
    pub fn starter_default(windivert_dir: impl Into<String>) -> Self {
        let mut proxies = HashMap::new();
        proxies.insert(
            "local_socks".to_string(),
            ProxyConfig {
                kind: ProxyKind::Socks5,
                address: "127.0.0.1:10808".to_string(),
                username: None,
                password: None,
                udp: true,
            },
        );
        let rules = vec![builtin_preset_rule("antigravity", "local_socks")
            .expect("built-in antigravity preset must exist")];
        Self {
            general: GeneralConfig {
                windivert_dir: windivert_dir.into(),
                ..GeneralConfig::default()
            },
            proxies,
            rules,
        }
    }

    pub fn validate(&self) -> Result<()> {
        for rule in &self.rules {
            if !rule.enabled {
                continue;
            }
            if rule.action == RuleAction::Proxy {
                let proxy_name = rule.proxy.as_deref().ok_or_else(|| {
                    anyhow!("rule '{}' has action='proxy' but no proxy name", rule.name)
                })?;
                if !self.proxies.contains_key(proxy_name) {
                    bail!(
                        "rule '{}' references unknown proxy '{}'",
                        rule.name,
                        proxy_name
                    );
                }
            }
        }
        Ok(())
    }
}

/// Metadata describing a built-in application preset for CLI and GUI catalogs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetInfo {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub process_names: Vec<String>,
    pub path_contains: Vec<String>,
    pub include_children: bool,
}

/// Returns all built-in application presets in display order.
pub fn list_builtin_presets() -> Vec<PresetInfo> {
    vec![
        PresetInfo {
            id: "antigravity".into(),
            name: "Antigravity".into(),
            category: "AI & IDE".into(),
            description: "Full Electron shell, language_server.exe, and spawned MCP/CLI child processes.".into(),
            process_names: vec![
                "Antigravity.exe".into(),
                "language_server.exe".into(),
                "language_server_windows_x64.exe".into(),
                "ls_core.exe".into(),
            ],
            path_contains: vec!["Antigravity".into()],
            include_children: true,
        },
        PresetInfo {
            id: "cursor".into(),
            name: "Cursor IDE".into(),
            category: "AI & IDE".into(),
            description: "Cursor editor, extension host, and spawned AI/language server subprocesses.".into(),
            process_names: vec!["Cursor.exe".into()],
            path_contains: vec!["cursor".into()],
            include_children: true,
        },
        PresetInfo {
            id: "vscode".into(),
            name: "VS Code".into(),
            category: "AI & IDE".into(),
            description: "Visual Studio Code, Copilot/extension hosts, and remote CLI workers.".into(),
            process_names: vec!["Code.exe".into()],
            path_contains: vec!["Microsoft VS Code".into()],
            include_children: true,
        },
        PresetInfo {
            id: "git".into(),
            name: "Git & SSH CLI".into(),
            category: "Developer".into(),
            description: "Git, git-remote-https, OpenSSH, and curl CLI operations.".into(),
            process_names: vec![
                "git.exe".into(),
                "git-remote-https.exe".into(),
                "ssh.exe".into(),
                "curl.exe".into(),
            ],
            path_contains: vec![],
            include_children: true,
        },
        PresetInfo {
            id: "discord".into(),
            name: "Discord".into(),
            category: "Messaging".into(),
            description: "Discord client, updater, and voice/gateway subprocesses.".into(),
            process_names: vec!["Discord.exe".into(), "Update.exe".into()],
            path_contains: vec!["Discord".into()],
            include_children: true,
        },
        PresetInfo {
            id: "telegram".into(),
            name: "Telegram Desktop".into(),
            category: "Messaging".into(),
            description: "Telegram Desktop native client.".into(),
            process_names: vec!["Telegram.exe".into()],
            path_contains: vec!["Telegram".into()],
            include_children: true,
        },
        PresetInfo {
            id: "steam".into(),
            name: "Steam".into(),
            category: "Gaming".into(),
            description: "Steam client, steamwebhelper, and launcher subprocesses.".into(),
            process_names: vec!["steam.exe".into(), "steamwebhelper.exe".into()],
            path_contains: vec!["Steam".into()],
            include_children: true,
        },
    ]
}

/// Returns `true` when portable mode is active (a `portable.json` marker or `./bypath-data`
/// folder exists next to the current executable).
pub fn is_portable_mode() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(dir) = exe.parent() else {
        return false;
    };
    let marker = dir.join("portable.json");
    if marker.exists() {
        if let Ok(raw) = std::fs::read_to_string(&marker) {
            if raw.contains("false") && !raw.contains("true") {
                return false;
            }
        }
        return true;
    }
    dir.join("bypath-data").is_dir()
}

/// Resolves the persistent data directory (`./bypath-data` in portable mode, or `%ProgramData%\Bypath`).
pub fn resolve_data_dir() -> PathBuf {
    if is_portable_mode() {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                return dir.join("bypath-data");
            }
        }
    }
    #[cfg(windows)]
    {
        if let Ok(program_data) = std::env::var("ProgramData") {
            return PathBuf::from(program_data).join("Bypath");
        }
    }
    PathBuf::from(".")
}

/// Returns the default persistent config path shared by the Windows Service, CLI, and GUI.
pub fn default_config_path() -> PathBuf {
    resolve_data_dir().join("config.toml")
}

/// Returns the default rolling log file path used by the background service and daemon.
pub fn default_log_path() -> PathBuf {
    resolve_data_dir().join("logs").join("bypath.log")
}

/// Resolves the most likely directory containing `WinDivert.dll` and `WinDivert64.sys`.
pub fn resolve_windivert_dir(configured: Option<&str>) -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let trimmed = configured.map(str::trim).filter(|s| !s.is_empty());
    let is_default_rel = matches!(trimmed, Some("runtime/windivert") | Some(r"runtime\windivert"));

    if let Some(c) = trimmed {
        if !is_default_rel {
            candidates.push(PathBuf::from(c));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Check bundled release path `<exe_dir>/runtime/windivert` BEFORE relative paths
            // so Windows Services (whose CWD is C:\Windows\System32) always find bundled drivers.
            candidates.push(dir.join("runtime").join("windivert"));
            candidates.push(dir.to_path_buf());
            candidates.push(dir.join("windivert"));
            // Walk up from target/release or apps/desktop/src-tauri/target/release during dev
            for ancestor in dir.ancestors().skip(1).take(8) {
                candidates.push(ancestor.join("runtime").join("windivert"));
            }
        }
    }
    candidates.push(resolve_data_dir().join("windivert"));
    candidates.push(PathBuf::from("runtime").join("windivert"));
    #[cfg(windows)]
    if let Ok(program_data) = std::env::var("ProgramData") {
        candidates.push(
            PathBuf::from(program_data)
                .join("Bypath")
                .join("windivert"),
        );
    }

    for cand in &candidates {
        if cand.join("WinDivert.dll").exists() {
            return cand.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("runtime").join("windivert"))
}

/// Returns a built-in rule template for well-known multi-process desktop applications.
pub fn builtin_preset_rule(preset_name: &str, proxy_name: &str) -> Result<RuleConfig> {
    let key = preset_name.trim().to_ascii_lowercase();
    let preset = list_builtin_presets()
        .into_iter()
        .find(|p| p.id == key)
        .ok_or_else(|| {
            let names: Vec<String> = list_builtin_presets().into_iter().map(|p| p.id).collect();
            anyhow!(
                "unknown preset '{key}'. Available presets: {}",
                names.join(", ")
            )
        })?;

    Ok(RuleConfig {
        name: format!("Preset: {}", preset.name),
        enabled: true,
        process_names: preset.process_names,
        path_prefixes: vec![],
        path_contains: preset.path_contains,
        include_children: preset.include_children,
        dst_cidrs: vec![],
        dst_ports: vec![],
        protocols: vec![],
        domains: vec![],
        action: RuleAction::Proxy,
        proxy: Some(proxy_name.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_proxy_uris() {
        let p1 = ProxyConfig::from_uri("socks5://127.0.0.1:7890", true).unwrap();
        assert_eq!(p1.kind, ProxyKind::Socks5);
        assert_eq!(p1.address, "127.0.0.1:7890");
        assert!(p1.udp);
        assert!(p1.username.is_none());

        let p2 = ProxyConfig::from_uri("http://alice:secret@127.0.0.1:8080/", true).unwrap();
        assert_eq!(p2.kind, ProxyKind::Http);
        assert_eq!(p2.address, "127.0.0.1:8080");
        assert_eq!(p2.username.as_deref(), Some("alice"));
        assert_eq!(p2.password.as_deref(), Some("secret"));
        assert!(!p2.udp);
    }

    #[test]
    fn builtin_antigravity_preset_valid() {
        let rule = builtin_preset_rule("antigravity", "default").unwrap();
        assert!(rule.include_children);
        assert!(rule
            .process_names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("Antigravity.exe")));
        assert!(rule
            .process_names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("language_server_windows_x64.exe")));
    }
}
