use crate::config::{ProxyConfig, ProxyKind};
use serde::{Deserialize, Serialize};

/// Snapshot of the OS system proxy or proxy environment variables.
/// Read only on explicit user action or Agent API calls (never overwrites config silently).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SystemProxyInfo {
    pub enabled: bool,
    /// Origin of the detected proxy: `"windows_registry"`, `"env"`, or `"none"`.
    pub source: String,
    pub raw_server: Option<String>,
    pub socks_address: Option<String>,
    pub http_address: Option<String>,
    pub bypass_list: Option<String>,
    pub suggested_kind: Option<ProxyKind>,
    pub suggested_address: Option<String>,
}

/// Reads the current Windows Internet Settings proxy configuration, falling back to
/// standard proxy environment variables (`ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`).
pub fn read_system_proxy() -> SystemProxyInfo {
    #[cfg(windows)]
    {
        if let Some(info) = read_windows_registry_proxy() {
            if info.enabled && info.suggested_address.is_some() {
                return info;
            }
        }
    }

    if let Some(info) = read_env_proxy() {
        return info;
    }

    SystemProxyInfo {
        enabled: false,
        source: "none".to_string(),
        ..Default::default()
    }
}

/// Parses a Windows `Internet Settings\ProxyServer` registry value into structured endpoints.
/// Supports both bare `host:port` (or `scheme://host:port`) and semicolon-delimited mappings
/// such as `http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7891`.
pub fn parse_windows_proxy_server(raw: &str, bypass_list: Option<String>) -> SystemProxyInfo {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return SystemProxyInfo {
            enabled: false,
            source: "windows_registry".to_string(),
            raw_server: Some(trimmed.to_string()),
            bypass_list,
            ..Default::default()
        };
    }

    let mut socks_address: Option<String> = None;
    let mut http_address: Option<String> = None;

    if trimmed.contains('=') {
        for part in trimmed.split(';').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some((proto, value)) = part.split_once('=') {
                let addr = normalize_host_port(value);
                if addr.is_empty() {
                    continue;
                }
                match proto.trim().to_ascii_lowercase().as_str() {
                    "socks" | "socks5" | "socks5h" => {
                        socks_address = Some(addr);
                    }
                    "http" | "https"
                        if http_address.is_none() || proto.eq_ignore_ascii_case("https") =>
                    {
                        http_address = Some(addr);
                    }
                    _ => {}
                }
            }
        }
    } else if let Some((scheme, rest)) = trimmed.split_once("://") {
        let addr = normalize_host_port(rest);
        if !addr.is_empty() {
            match scheme.trim().to_ascii_lowercase().as_str() {
                "socks" | "socks5" | "socks5h" => socks_address = Some(addr),
                "http" | "https" => http_address = Some(addr),
                _ => http_address = Some(addr),
            }
        }
    } else {
        let addr = normalize_host_port(trimmed);
        if !addr.is_empty() {
            http_address = Some(addr);
        }
    }

    let (suggested_kind, suggested_address) = if let Some(ref socks) = socks_address {
        (Some(ProxyKind::Socks5), Some(socks.clone()))
    } else if let Some(ref http) = http_address {
        (Some(ProxyKind::Http), Some(http.clone()))
    } else {
        (None, None)
    };

    SystemProxyInfo {
        enabled: suggested_address.is_some(),
        source: "windows_registry".to_string(),
        raw_server: Some(trimmed.to_string()),
        socks_address,
        http_address,
        bypass_list,
        suggested_kind,
        suggested_address,
    }
}

fn normalize_host_port(raw: &str) -> String {
    let mut s = raw.trim();
    if let Some((_, rest)) = s.split_once("://") {
        s = rest;
    }
    if let Some((_, after_auth)) = s.rsplit_once('@') {
        s = after_auth;
    }
    s = s.trim_end_matches('/');
    if !s.contains(':') {
        return String::new();
    }
    s.to_string()
}

fn read_env_proxy() -> Option<SystemProxyInfo> {
    let vars = [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ];
    for key in vars {
        if let Ok(val) = std::env::var(key) {
            let trimmed = val.trim();
            if trimmed.is_empty() {
                continue;
            }
            let uri = if trimmed.contains("://") {
                trimmed.to_string()
            } else {
                format!("http://{trimmed}")
            };
            if let Ok(cfg) = ProxyConfig::from_uri(&uri, true) {
                let (socks_address, http_address) = match cfg.kind {
                    ProxyKind::Socks5 => (Some(cfg.address.clone()), None),
                    ProxyKind::Http => (None, Some(cfg.address.clone())),
                };
                return Some(SystemProxyInfo {
                    enabled: true,
                    source: "env".to_string(),
                    raw_server: Some(format!("{key}={trimmed}")),
                    socks_address,
                    http_address,
                    bypass_list: std::env::var("NO_PROXY")
                        .or_else(|_| std::env::var("no_proxy"))
                        .ok(),
                    suggested_kind: Some(cfg.kind),
                    suggested_address: Some(cfg.address),
                });
            }
        }
    }
    None
}

#[cfg(windows)]
fn read_windows_registry_proxy() -> Option<SystemProxyInfo> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD,
        REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE,
    };

    const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    let subkey_w: Vec<u16> = SUBKEY.encode_utf16().chain(std::iter::once(0)).collect();
    let mut hkey = HKEY::default();
    let open_status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        )
    };
    if open_status.is_err() {
        return None;
    }

    let enabled = query_reg_dword(hkey, "ProxyEnable").unwrap_or(0) != 0;
    let raw_server = query_reg_string(hkey, "ProxyServer");
    let bypass_list = query_reg_string(hkey, "ProxyOverride");
    unsafe {
        let _ = RegCloseKey(hkey);
    }

    if !enabled {
        return Some(SystemProxyInfo {
            enabled: false,
            source: "windows_registry".to_string(),
            raw_server,
            bypass_list,
            ..Default::default()
        });
    }

    let server = raw_server?;
    return Some(parse_windows_proxy_server(&server, bypass_list));

    fn query_reg_dword(hkey: HKEY, value_name: &str) -> Option<u32> {
        let name_w: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut kind = REG_VALUE_TYPE::default();
        let mut data = 0u32;
        let mut len = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                Some(&mut kind),
                Some((&mut data as *mut u32).cast::<u8>()),
                Some(&mut len),
            )
        };
        if status.is_ok() && kind == REG_DWORD {
            Some(data)
        } else {
            None
        }
    }

    fn query_reg_string(hkey: HKEY, value_name: &str) -> Option<String> {
        let name_w: Vec<u16> = value_name.encode_utf16().chain(std::iter::once(0)).collect();
        let mut kind = REG_VALUE_TYPE::default();
        let mut len = 0u32;
        let first = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut len),
            )
        };
        if first.is_err() || len == 0 || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let second = unsafe {
            RegQueryValueExW(
                hkey,
                PCWSTR(name_w.as_ptr()),
                None,
                Some(&mut kind),
                Some(buf.as_mut_ptr()),
                Some(&mut len),
            )
        };
        if second.is_err() {
            return None;
        }
        let words: Vec<u16> = buf[..len as usize]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let end = words.iter().position(|&c| c == 0).unwrap_or(words.len());
        let s = String::from_utf16_lossy(&words[..end]).trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_windows_proxy_server() {
        let info = parse_windows_proxy_server("127.0.0.1:7897", Some("localhost;<local>".into()));
        assert!(info.enabled);
        assert_eq!(info.http_address.as_deref(), Some("127.0.0.1:7897"));
        assert_eq!(info.socks_address, None);
        assert_eq!(info.suggested_kind, Some(ProxyKind::Http));
        assert_eq!(info.suggested_address.as_deref(), Some("127.0.0.1:7897"));
    }

    #[test]
    fn parses_multi_protocol_windows_proxy_server() {
        let info = parse_windows_proxy_server(
            "http=127.0.0.1:10809;https=127.0.0.1:10809;socks=127.0.0.1:10808",
            None,
        );
        assert!(info.enabled);
        assert_eq!(info.http_address.as_deref(), Some("127.0.0.1:10809"));
        assert_eq!(info.socks_address.as_deref(), Some("127.0.0.1:10808"));
        assert_eq!(info.suggested_kind, Some(ProxyKind::Socks5));
        assert_eq!(info.suggested_address.as_deref(), Some("127.0.0.1:10808"));
    }

    #[test]
    fn parses_scheme_prefixed_proxy_server() {
        let info = parse_windows_proxy_server("socks5://127.0.0.1:10808/", None);
        assert!(info.enabled);
        assert_eq!(info.socks_address.as_deref(), Some("127.0.0.1:10808"));
        assert_eq!(info.suggested_kind, Some(ProxyKind::Socks5));
    }

    #[test]
    fn handles_empty_or_invalid_server_gracefully() {
        let info = parse_windows_proxy_server("   ", None);
        assert!(!info.enabled);
        assert!(info.suggested_address.is_none());
    }
}
