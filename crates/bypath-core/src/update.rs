use crate::config::{Config, ProxyConfig, ProxyKind, RuleAction};
use crate::upstream::system_proxy::read_system_proxy;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashSet;
use std::process::Stdio;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub const GITHUB_REPO: &str = "Nexusyyan/bypath";
pub const LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/Nexusyyan/bypath/releases/latest";
pub const RELEASES_PAGE_URL: &str = "https://github.com/Nexusyyan/bypath/releases";
pub const ALLOWED_EXTERNAL_PREFIX: &str = "https://github.com/Nexusyyan/bypath";
pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_RELEASE_NOTES_CHARS: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckReport {
    pub current_version: String,
    pub latest_version: String,
    pub latest_tag: String,
    pub update_available: bool,
    pub release_name: String,
    pub release_url: String,
    pub published_at: Option<String>,
    pub release_notes: Option<String>,
    pub primary_asset_name: Option<String>,
    pub primary_asset_url: Option<String>,
    pub primary_asset_size: Option<u64>,
    pub nodriver_asset_name: Option<String>,
    pub nodriver_asset_url: Option<String>,
    pub checked_via: String,
    pub checked_at_ms: u64,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseResponse {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SemVer {
    major: u64,
    minor: u64,
    patch: u64,
    prerelease: Option<String>,
}

impl SemVer {
    fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let stripped = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        let without_build = stripped.split('+').next()?.trim();
        if without_build.is_empty() {
            return None;
        }
        let (core, prerelease) = match without_build.split_once('-') {
            Some((c, pre)) => (c, Some(pre.to_string()).filter(|s| !s.is_empty())),
            None => (without_build, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse::<u64>().ok()?;
        let minor = parts.next().unwrap_or("0").parse::<u64>().ok()?;
        let patch = parts.next().unwrap_or("0").parse::<u64>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            prerelease,
        })
    }

    fn normalized(&self) -> String {
        match &self.prerelease {
            Some(pre) => format!("{}.{}.{}-{pre}", self.major, self.minor, self.patch),
            None => format!("{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_prerelease(a, b),
            })
    }
}

impl PartialOrd for SemVer {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let mut a_parts = a.split('.');
    let mut b_parts = b.split('.');
    loop {
        match (a_parts.next(), b_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ap), Some(bp)) => {
                let ord = match (ap.parse::<u64>(), bp.parse::<u64>()) {
                    (Ok(an), Ok(bn)) => an.cmp(&bn),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => ap.cmp(bp),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Returns `true` when `candidate` is strictly newer than `current` according to SemVer 2.0.0.
pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    match (SemVer::parse(current), SemVer::parse(candidate)) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}

/// Validates that an external URL points strictly to this repository on GitHub over HTTPS.
pub fn is_safe_release_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    trimmed == ALLOWED_EXTERNAL_PREFIX
        || trimmed.starts_with(&format!("{ALLOWED_EXTERNAL_PREFIX}/"))
}

fn truncate_notes(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().count() <= MAX_RELEASE_NOTES_CHARS {
        return Some(trimmed.to_string());
    }
    let truncated: String = trimmed.chars().take(MAX_RELEASE_NOTES_CHARS).collect();
    Some(format!("{truncated}..."))
}

/// Parses a GitHub `/releases/latest` JSON payload into a structured [`UpdateCheckReport`].
pub fn parse_release_json(
    current_version: &str,
    body: &str,
    checked_via: impl Into<String>,
) -> Result<UpdateCheckReport> {
    let parsed: GithubReleaseResponse =
        serde_json::from_str(body).context("failed to parse GitHub release JSON response")?;
    let latest_tag = parsed.tag_name.trim().to_string();
    if latest_tag.is_empty() {
        bail!("GitHub release response is missing tag_name");
    }
    let latest_version = SemVer::parse(&latest_tag)
        .map(|v| v.normalized())
        .unwrap_or_else(|| {
            latest_tag
                .strip_prefix('v')
                .or_else(|| latest_tag.strip_prefix('V'))
                .unwrap_or(&latest_tag)
                .to_string()
        });
    let current_norm = SemVer::parse(current_version)
        .map(|v| v.normalized())
        .unwrap_or_else(|| current_version.trim().to_string());
    let update_available = is_newer_version(&current_norm, &latest_version);

    let release_url = parsed
        .html_url
        .as_deref()
        .filter(|u| is_safe_release_url(u))
        .map(ToString::to_string)
        .unwrap_or_else(|| RELEASES_PAGE_URL.to_string());

    let release_name = parsed
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| latest_tag.clone());

    let mut primary_asset: Option<&GithubReleaseAsset> = None;
    let mut nodriver_asset: Option<&GithubReleaseAsset> = None;
    for asset in &parsed.assets {
        if !is_safe_release_url(&asset.browser_download_url) {
            continue;
        }
        let lower = asset.name.to_ascii_lowercase();
        if lower.ends_with("-windows-x64-nodriver.zip") {
            nodriver_asset = Some(asset);
        } else if lower.ends_with("-windows-x64.zip") && !lower.contains("nodriver") {
            primary_asset = Some(asset);
        }
    }

    let checked_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    Ok(UpdateCheckReport {
        current_version: current_norm,
        latest_version,
        latest_tag,
        update_available,
        release_name,
        release_url,
        published_at: parsed.published_at.filter(|s| !s.trim().is_empty()),
        release_notes: parsed.body.as_deref().and_then(truncate_notes),
        primary_asset_name: primary_asset.map(|a| a.name.clone()),
        primary_asset_url: primary_asset.map(|a| a.browser_download_url.clone()),
        primary_asset_size: primary_asset.and_then(|a| a.size),
        nodriver_asset_name: nodriver_asset.map(|a| a.name.clone()),
        nodriver_asset_url: nodriver_asset.map(|a| a.browser_download_url.clone()),
        checked_via: checked_via.into(),
        checked_at_ms,
    })
}

#[derive(Debug, Clone)]
struct FetchRoute {
    label: String,
    proxy_url: Option<String>,
    proxy_user: Option<String>,
}

fn build_fetch_routes(
    config: Option<&Config>,
    explicit_proxy: Option<&ProxyConfig>,
) -> Vec<FetchRoute> {
    if let Some(p) = explicit_proxy {
        return vec![route_from_proxy("proxy:explicit", p)];
    }

    let mut routes: Vec<FetchRoute> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();

    if let Some(cfg) = config {
        // Prioritize proxies referenced by enabled rules first.
        let mut ordered_names: Vec<String> = Vec::new();
        for rule in &cfg.rules {
            if rule.enabled && rule.action == RuleAction::Proxy {
                if let Some(ref name) = rule.proxy {
                    if cfg.proxies.contains_key(name) && !ordered_names.contains(name) {
                        ordered_names.push(name.clone());
                    }
                }
            }
        }
        let mut remaining: Vec<String> = cfg
            .proxies
            .keys()
            .filter(|k| !ordered_names.contains(*k))
            .cloned()
            .collect();
        remaining.sort();
        ordered_names.extend(remaining);

        for name in ordered_names {
            if let Some(proxy) = cfg.proxies.get(&name) {
                let route = route_from_proxy(&format!("proxy:{name}"), proxy);
                if let Some(ref url) = route.proxy_url {
                    if seen_urls.insert(url.clone()) {
                        routes.push(route);
                    }
                }
            }
        }
    }

    let sys = read_system_proxy();
    if sys.enabled {
        if let Some(addr) = sys.suggested_address {
            let scheme = match sys.suggested_kind.unwrap_or(ProxyKind::Http) {
                ProxyKind::Socks5 => "socks5h",
                ProxyKind::Http => "http",
            };
            let url = format!("{scheme}://{addr}");
            if seen_urls.insert(url.clone()) {
                routes.push(FetchRoute {
                    label: "system_proxy".to_string(),
                    proxy_url: Some(url),
                    proxy_user: None,
                });
            }
        }
    }

    routes.push(FetchRoute {
        label: "direct".to_string(),
        proxy_url: None,
        proxy_user: None,
    });
    routes
}

fn route_from_proxy(label: &str, proxy: &ProxyConfig) -> FetchRoute {
    let scheme = match proxy.kind {
        ProxyKind::Socks5 => "socks5h",
        ProxyKind::Http => "http",
    };
    let proxy_user = proxy.username.as_ref().and_then(|u| {
        let user = u.trim();
        if user.is_empty() {
            None
        } else {
            let pass = proxy.password.as_deref().unwrap_or("");
            Some(format!("{user}:{pass}"))
        }
    });
    FetchRoute {
        label: label.to_string(),
        proxy_url: Some(format!("{scheme}://{}", proxy.address.trim())),
        proxy_user,
    }
}

fn escape_curl_value(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    out.push('"');
    for ch in raw.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' | '\n' => {}
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

enum FetchOutcome {
    Ok(String),
    HttpError(u16, String),
    TransportError(String),
}

async fn fetch_latest_via_route(route: &FetchRoute, timeout: Duration) -> FetchOutcome {
    let total_secs = timeout.as_secs().max(3);
    let connect_secs = (total_secs / 2).clamp(2, 5);
    let curl_bin = if cfg!(windows) { "curl.exe" } else { "curl" };

    let mut cmd = Command::new(curl_bin);
    cmd.args([
        "-sS",
        "-L",
        "--connect-timeout",
        &connect_secs.to_string(),
        "--max-time",
        &total_secs.to_string(),
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "X-GitHub-Api-Version: 2022-11-28",
        "-H",
        &format!("User-Agent: Bypath/{CURRENT_VERSION}"),
        "-w",
        "\n%{http_code}",
        "--config",
        "-",
        LATEST_RELEASE_API_URL,
    ]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW prevents any transient console window flash in GUI builds.
        cmd.creation_flags(0x0800_0000);
    }

    let mut cfg_stdin = String::new();
    match &route.proxy_url {
        Some(url) => {
            cfg_stdin.push_str(&format!("proxy = {}\n", escape_curl_value(url)));
            if let Some(ref creds) = route.proxy_user {
                cfg_stdin.push_str(&format!("proxy-user = {}\n", escape_curl_value(creds)));
            }
        }
        None => {
            cfg_stdin.push_str("noproxy = \"*\"\n");
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => return FetchOutcome::TransportError(format!("failed to spawn {curl_bin}: {err}")),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(cfg_stdin.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let output = match tokio::time::timeout(timeout + Duration::from_secs(2), child.wait_with_output()).await {
        Ok(Ok(out)) => out,
        Ok(Err(err)) => return FetchOutcome::TransportError(err.to_string()),
        Err(_) => return FetchOutcome::TransportError("request timed out".to_string()),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if stderr.is_empty() {
            format!("curl exited with {}", output.status)
        } else {
            stderr
        };
        return FetchOutcome::TransportError(detail);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (body, code_str) = match stdout.rsplit_once('\n') {
        Some((b, c)) => (b, c.trim()),
        None => ("", stdout.trim()),
    };
    let status_code = code_str.parse::<u16>().unwrap_or(0);
    if status_code == 200 {
        FetchOutcome::Ok(body.to_string())
    } else {
        FetchOutcome::HttpError(status_code, body.trim().to_string())
    }
}

/// Checks GitHub Releases for a newer Bypath version, automatically utilizing
/// configured Bypath upstream proxies or the Windows system proxy before falling back to direct.
pub async fn check_for_updates(
    config: Option<&Config>,
    explicit_proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> Result<UpdateCheckReport> {
    let routes = build_fetch_routes(config, explicit_proxy);
    let mut errors: Vec<String> = Vec::new();

    for route in routes {
        match fetch_latest_via_route(&route, timeout).await {
            FetchOutcome::Ok(body) => {
                return parse_release_json(CURRENT_VERSION, &body, route.label);
            }
            FetchOutcome::HttpError(404, _) => {
                bail!("no published GitHub releases found for {GITHUB_REPO}");
            }
            FetchOutcome::HttpError(403, body) => {
                if body.to_ascii_lowercase().contains("rate limit") {
                    bail!("GitHub API rate limit exceeded; try again later or open {RELEASES_PAGE_URL}");
                }
                errors.push(format!("{}: HTTP 403", route.label));
            }
            FetchOutcome::HttpError(code, _) => {
                errors.push(format!("{}: HTTP {code}", route.label));
            }
            FetchOutcome::TransportError(msg) => {
                errors.push(format!("{}: {msg}", route.label));
            }
        }
    }

    Err(anyhow!(
        "failed to check updates via all routes ({})",
        errors.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison_handles_tags_and_prereleases() {
        assert!(!is_newer_version("0.4.1", "v0.4.1"));
        assert!(!is_newer_version("0.4.1", "v0.3.0"));
        assert!(is_newer_version("0.4.1", "v0.4.2"));
        assert!(is_newer_version("0.4.1", "0.5.0"));
        assert!(is_newer_version("0.4.1", "v1.0.0"));
        assert!(is_newer_version("0.5.0-beta.1", "v0.5.0"));
        assert!(!is_newer_version("0.5.0", "v0.5.0-rc.1"));
        assert!(is_newer_version("0.5.0-alpha.2", "0.5.0-alpha.10"));
        assert!(!is_newer_version("0.4.1", "not-a-version"));
    }

    #[test]
    fn validates_github_release_urls_strictly() {
        assert!(is_safe_release_url(
            "https://github.com/Nexusyyan/bypath/releases/tag/v0.4.1"
        ));
        assert!(is_safe_release_url(
            "https://github.com/Nexusyyan/bypath/releases/download/v0.4.1/bypath-v0.4.1-windows-x64.zip"
        ));
        assert!(!is_safe_release_url("https://evil.example.com/bypath.zip"));
        assert!(!is_safe_release_url("http://github.com/Nexusyyan/bypath/releases"));
        assert!(!is_safe_release_url(
            "https://github.com/Nexusyyan/bypath-evil/releases"
        ));
        assert!(!is_safe_release_url(
            "https://github.com/Nexusyyan/bypath/releases\ncalc.exe"
        ));
    }

    #[test]
    fn parses_github_release_payload_and_selects_dual_zip_assets() {
        let json = r###"{
            "tag_name": "v0.5.0",
            "name": "Bypath v0.5.0",
            "html_url": "https://github.com/Nexusyyan/bypath/releases/tag/v0.5.0",
            "published_at": "2026-11-30T10:00:00Z",
            "body": "## Highlights\n- Faster engine\n- Update check",
            "assets": [
                {
                    "name": "bypath-v0.5.0-windows-x64-nodriver.zip",
                    "browser_download_url": "https://github.com/Nexusyyan/bypath/releases/download/v0.5.0/bypath-v0.5.0-windows-x64-nodriver.zip",
                    "size": 8100000
                },
                {
                    "name": "bypath-v0.5.0-windows-x64.zip",
                    "browser_download_url": "https://github.com/Nexusyyan/bypath/releases/download/v0.5.0/bypath-v0.5.0-windows-x64.zip",
                    "size": 8400000
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": "https://github.com/Nexusyyan/bypath/releases/download/v0.5.0/SHA256SUMS.txt",
                    "size": 512
                }
            ]
        }"###;

        let report = parse_release_json("0.4.1", json, "proxy:local_socks").unwrap();
        assert_eq!(report.current_version, "0.4.1");
        assert_eq!(report.latest_version, "0.5.0");
        assert_eq!(report.latest_tag, "v0.5.0");
        assert!(report.update_available);
        assert_eq!(
            report.primary_asset_name.as_deref(),
            Some("bypath-v0.5.0-windows-x64.zip")
        );
        assert_eq!(report.primary_asset_size, Some(8_400_000));
        assert_eq!(
            report.nodriver_asset_name.as_deref(),
            Some("bypath-v0.5.0-windows-x64-nodriver.zip")
        );
        assert_eq!(report.checked_via, "proxy:local_socks");
    }

    #[test]
    fn parse_release_rejects_untrusted_asset_urls() {
        let json = r#"{
            "tag_name": "v0.4.1",
            "html_url": "https://evil.example.com/phish",
            "assets": [
                {
                    "name": "bypath-v0.4.1-windows-x64.zip",
                    "browser_download_url": "https://evil.example.com/bypath-v0.4.1-windows-x64.zip",
                    "size": 123
                }
            ]
        }"#;

        let report = parse_release_json("0.4.1", json, "direct").unwrap();
        assert!(!report.update_available);
        assert_eq!(report.release_url, RELEASES_PAGE_URL);
        assert!(report.primary_asset_url.is_none());
    }
}
