use crate::config::{Config, ProxyConfig, ProxyKind, RuleAction};
use crate::upstream::system_proxy::read_system_proxy;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
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
const MAX_EXTRACT_FILES: usize = 256;
const MAX_EXTRACT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const MIN_GUI_EXE_BYTES: u64 = 64 * 1024;

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
    #[serde(default)]
    pub nodriver_asset_size: Option<u64>,
    #[serde(default)]
    pub gui_exe_asset_name: Option<String>,
    #[serde(default)]
    pub gui_exe_asset_url: Option<String>,
    #[serde(default)]
    pub gui_exe_asset_size: Option<u64>,
    #[serde(default)]
    pub checksums_asset_url: Option<String>,
    pub checked_via: String,
    pub checked_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateAssetKind {
    ZipBundle,
    StandaloneGuiExe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedUpdateAsset {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    pub kind: UpdateAssetKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateProgressEvent {
    pub stage: String,
    pub version: String,
    pub tag: String,
    pub asset_name: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: u8,
    pub route: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedUpdate {
    pub target_version: String,
    pub target_tag: String,
    pub asset_name: String,
    pub asset_kind: UpdateAssetKind,
    pub staging_root: PathBuf,
    pub payload_dir: PathBuf,
    pub sha256: String,
    pub downloaded_via: String,
    pub staged_at_ms: u64,
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

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    let mut gui_exe_asset: Option<&GithubReleaseAsset> = None;
    let mut checksums_asset: Option<&GithubReleaseAsset> = None;

    for asset in &parsed.assets {
        if !is_safe_release_url(&asset.browser_download_url) {
            continue;
        }
        let lower = asset.name.to_ascii_lowercase();
        if lower == "sha256sums.txt" {
            checksums_asset = Some(asset);
        } else if lower.ends_with("-windows-x64-nodriver.zip") {
            nodriver_asset = Some(asset);
        } else if lower.ends_with("-windows-x64.zip") && !lower.contains("nodriver") {
            primary_asset = Some(asset);
        } else if lower.starts_with("bypath-gui-") && lower.ends_with("-windows-x64.exe") {
            gui_exe_asset = Some(asset);
        }
    }

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
        nodriver_asset_size: nodriver_asset.and_then(|a| a.size),
        gui_exe_asset_name: gui_exe_asset.map(|a| a.name.clone()),
        gui_exe_asset_url: gui_exe_asset.map(|a| a.browser_download_url.clone()),
        gui_exe_asset_size: gui_exe_asset.and_then(|a| a.size),
        checksums_asset_url: checksums_asset.map(|a| a.browser_download_url.clone()),
        checked_via: checked_via.into(),
        checked_at_ms: now_epoch_ms(),
    })
}

/// Chooses the best release artifact for in-app updates:
/// - When signed WinDivert files already exist locally, prefer the clean no-driver ZIP so
///   existing AV-allowed driver binaries are preserved untouched and download size stays small.
/// - When WinDivert is missing locally, prefer the full out-of-the-box ZIP to restore it.
/// - Fall back to the standalone GUI executable if neither ZIP asset is published.
pub fn select_update_asset(
    report: &UpdateCheckReport,
    has_local_windivert: bool,
) -> Option<SelectedUpdateAsset> {
    let primary = report
        .primary_asset_name
        .as_ref()
        .zip(report.primary_asset_url.as_ref())
        .filter(|(_, url)| is_safe_release_url(url))
        .map(|(name, url)| SelectedUpdateAsset {
            name: name.clone(),
            url: url.clone(),
            size: report.primary_asset_size,
            kind: UpdateAssetKind::ZipBundle,
        });

    let nodriver = report
        .nodriver_asset_name
        .as_ref()
        .zip(report.nodriver_asset_url.as_ref())
        .filter(|(_, url)| is_safe_release_url(url))
        .map(|(name, url)| SelectedUpdateAsset {
            name: name.clone(),
            url: url.clone(),
            size: report.nodriver_asset_size,
            kind: UpdateAssetKind::ZipBundle,
        });

    let standalone = report
        .gui_exe_asset_name
        .as_ref()
        .zip(report.gui_exe_asset_url.as_ref())
        .filter(|(_, url)| is_safe_release_url(url))
        .map(|(name, url)| SelectedUpdateAsset {
            name: name.clone(),
            url: url.clone(),
            size: report.gui_exe_asset_size,
            kind: UpdateAssetKind::StandaloneGuiExe,
        });

    if has_local_windivert {
        nodriver.or(primary).or(standalone)
    } else {
        primary.or(nodriver).or(standalone)
    }
}

/// Parses a `SHA256SUMS.txt` file into a map of lowercase filename -> 64-char lowercase hex digest.
pub fn parse_sha256sums(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(hash_raw) = parts.next() else {
            continue;
        };
        let Some(name_raw) = parts.next() else {
            continue;
        };
        let hash = hash_raw.trim().to_ascii_lowercase();
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let name = name_raw.trim().trim_start_matches('*').trim();
        if name.is_empty() {
            continue;
        }
        map.insert(name.to_ascii_lowercase(), hash);
    }
    map
}

/// Computes the lowercase hex SHA-256 digest of a file using streaming reads.
pub fn compute_file_sha256(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open '{}' for hashing", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(hex)
}

/// Verifies a file's SHA-256 digest against the expected 64-char hex string.
pub fn verify_file_sha256(path: &Path, expected_hex: &str) -> Result<String> {
    let expected = expected_hex.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("invalid expected SHA-256 digest format");
    }
    let actual = compute_file_sha256(path)?;
    if actual != expected {
        bail!(
            "SHA-256 checksum mismatch for '{}' (expected {}, got {})",
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string()),
            expected,
            actual
        );
    }
    Ok(actual)
}

/// Validates that a staged executable starts with the Windows PE `MZ` signature and meets `min_bytes`.
pub fn verify_pe_executable(path: &Path, min_bytes: u64) -> Result<()> {
    let meta = fs::metadata(path)
        .with_context(|| format!("missing staged executable '{}'", path.display()))?;
    if !meta.is_file() || meta.len() < min_bytes {
        bail!(
            "staged executable '{}' is unexpectedly small ({} bytes)",
            path.display(),
            meta.len()
        );
    }
    let mut file = File::open(path)?;
    let mut magic = [0u8; 2];
    file.read_exact(&mut magic)
        .context("failed to read PE header magic bytes")?;
    if &magic != b"MZ" {
        bail!("staged executable '{}' is not a valid Windows PE binary", path.display());
    }
    Ok(())
}

/// Safely extracts a ZIP archive into `dest_dir`, rejecting path traversal (Zip Slip)
/// and excessive uncompressed size (Zip Bomb). Returns the resolved payload root containing
/// `bypath-gui.exe` (supporting both flat archives and single-top-folder archives).
pub fn extract_zip_safely(zip_path: &Path, dest_dir: &Path) -> Result<PathBuf> {
    let file = File::open(zip_path)
        .with_context(|| format!("failed to open ZIP archive '{}'", zip_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("failed to read ZIP archive")?;

    if archive.len() > MAX_EXTRACT_FILES {
        bail!(
            "refused to extract update archive with too many entries ({})",
            archive.len()
        );
    }

    fs::create_dir_all(dest_dir)
        .with_context(|| format!("failed to create extract directory '{}'", dest_dir.display()))?;

    let mut total_written = 0u64;
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx)?;
        let Some(rel_path) = entry.enclosed_name() else {
            bail!("refused unsafe archive entry path: '{}'", entry.name());
        };
        if rel_path.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest_dir.join(&rel_path);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out_file = File::create(&out_path)?;
        let mut limited = entry.by_ref().take(MAX_EXTRACT_TOTAL_BYTES - total_written + 1);
        let written = io::copy(&mut limited, &mut out_file)?;
        total_written = total_written.saturating_add(written);
        if total_written > MAX_EXTRACT_TOTAL_BYTES {
            bail!("update archive exceeds maximum safe uncompressed size");
        }
    }

    resolve_extracted_payload_dir(dest_dir)
}

fn resolve_extracted_payload_dir(dest_dir: &Path) -> Result<PathBuf> {
    if dest_dir.join("bypath-gui.exe").is_file() {
        return Ok(dest_dir.to_path_buf());
    }
    let mut subdirs = Vec::new();
    for entry in fs::read_dir(dest_dir)? {
        let entry = entry?;
        if entry.path().is_dir() {
            subdirs.push(entry.path());
        }
    }
    if subdirs.len() == 1 && subdirs[0].join("bypath-gui.exe").is_file() {
        return Ok(subdirs.remove(0));
    }
    bail!("extracted update archive does not contain bypath-gui.exe")
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
    preferred_label: Option<&str>,
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

    if let Some(pref) = preferred_label {
        if let Some(pos) = routes.iter().position(|r| r.label == pref) {
            if pos > 0 {
                let preferred = routes.remove(pos);
                routes.insert(0, preferred);
            }
        }
    }

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

fn build_curl_proxy_config(route: &FetchRoute) -> String {
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
    cfg_stdin
}

enum FetchOutcome {
    Ok(String),
    HttpError(u16, String),
    TransportError(String),
}

async fn fetch_text_via_route(
    url: &str,
    accept_header: Option<&str>,
    route: &FetchRoute,
    timeout: Duration,
) -> FetchOutcome {
    let total_secs = timeout.as_secs().max(3);
    let connect_secs = (total_secs / 2).clamp(2, 8);
    let curl_bin = if cfg!(windows) { "curl.exe" } else { "curl" };

    let mut args = vec![
        "-sS".to_string(),
        "-L".to_string(),
        "--connect-timeout".to_string(),
        connect_secs.to_string(),
        "--max-time".to_string(),
        total_secs.to_string(),
        "-H".to_string(),
        format!("User-Agent: Bypath/{CURRENT_VERSION}"),
    ];
    if let Some(accept) = accept_header {
        args.push("-H".to_string());
        args.push(format!("Accept: {accept}"));
        args.push("-H".to_string());
        args.push("X-GitHub-Api-Version: 2022-11-28".to_string());
    }
    args.extend([
        "-w".to_string(),
        "\n%{http_code}".to_string(),
        "--config".to_string(),
        "-".to_string(),
        url.to_string(),
    ]);

    let mut cmd = Command::new(curl_bin);
    cmd.args(&args);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW prevents any transient console window flash in GUI builds.
        cmd.creation_flags(0x0800_0000);
    }

    let cfg_stdin = build_curl_proxy_config(route);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => return FetchOutcome::TransportError(format!("failed to spawn {curl_bin}: {err}")),
    };

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(cfg_stdin.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let output =
        match tokio::time::timeout(timeout + Duration::from_secs(2), child.wait_with_output()).await
        {
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

async fn download_file_via_route<F>(
    url: &str,
    dest_part: &Path,
    expected_size: Option<u64>,
    route: &FetchRoute,
    timeout: Duration,
    cancel: &Arc<AtomicBool>,
    on_bytes: &F,
) -> Result<()>
where
    F: Fn(u64, Option<u64>),
{
    let _ = fs::remove_file(dest_part);
    let total_secs = timeout.as_secs().max(30);
    let connect_secs = 12u64;
    let curl_bin = if cfg!(windows) { "curl.exe" } else { "curl" };

    let mut cmd = Command::new(curl_bin);
    cmd.args([
        "-sS",
        "-L",
        "--fail",
        "--connect-timeout",
        &connect_secs.to_string(),
        "--max-time",
        &total_secs.to_string(),
        "-H",
        &format!("User-Agent: Bypath/{CURRENT_VERSION}"),
        "-o",
        &dest_part.display().to_string(),
        "--config",
        "-",
        url,
    ]);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000);
    }

    let cfg_stdin = build_curl_proxy_config(route);
    let mut child = cmd
        .spawn()
        .with_context(|| format!("failed to spawn {curl_bin} for update download"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(cfg_stdin.as_bytes()).await;
        let _ = stdin.shutdown().await;
    }

    let stderr_handle = child.stderr.take().map(|mut err_pipe| {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            let _ = err_pipe.read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).trim().to_string()
        })
    });

    let deadline = tokio::time::Instant::now() + timeout + Duration::from_secs(5);
    let mut last_reported = 0u64;
    let status = loop {
        if cancel.load(AtomicOrdering::SeqCst) {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = fs::remove_file(dest_part);
            bail!("UPDATE_CANCELLED");
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = fs::remove_file(dest_part);
            bail!("download timed out");
        }
        if let Ok(meta) = fs::metadata(dest_part) {
            let cur = meta.len();
            if cur != last_reported {
                last_reported = cur;
                on_bytes(cur, expected_size);
            }
        }
        if let Some(exit_status) = child.try_wait()? {
            break exit_status;
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    };

    if let Ok(meta) = fs::metadata(dest_part) {
        on_bytes(meta.len(), expected_size);
    }

    let stderr_msg = match stderr_handle {
        Some(h) => h.await.unwrap_or_default(),
        None => String::new(),
    };

    if !status.success() {
        let _ = fs::remove_file(dest_part);
        if stderr_msg.is_empty() {
            bail!("curl exited with {status}");
        }
        bail!("{stderr_msg}");
    }

    let meta = fs::metadata(dest_part).context("downloaded file missing after curl completed")?;
    if meta.len() == 0 {
        let _ = fs::remove_file(dest_part);
        bail!("downloaded update artifact is empty");
    }
    Ok(())
}

fn compute_percent(downloaded: u64, total: Option<u64>) -> u8 {
    match total {
        Some(t) if t > 0 => ((downloaded.saturating_mul(100)) / t).min(100) as u8,
        _ => 0,
    }
}

/// Checks GitHub Releases for a newer Bypath version, automatically utilizing
/// configured Bypath upstream proxies or the Windows system proxy before falling back to direct.
pub async fn check_for_updates(
    config: Option<&Config>,
    explicit_proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> Result<UpdateCheckReport> {
    let routes = build_fetch_routes(config, explicit_proxy, None);
    let mut errors: Vec<String> = Vec::new();

    for route in routes {
        match fetch_text_via_route(
            LATEST_RELEASE_API_URL,
            Some("application/vnd.github+json"),
            &route,
            timeout,
        )
        .await
        {
            FetchOutcome::Ok(body) => {
                return parse_release_json(CURRENT_VERSION, &body, route.label);
            }
            FetchOutcome::HttpError(404, _) => {
                bail!("no published GitHub releases found for {GITHUB_REPO}");
            }
            FetchOutcome::HttpError(403, body) => {
                if body.to_ascii_lowercase().contains("rate limit") {
                    bail!(
                        "GitHub API rate limit exceeded; try again later or open {RELEASES_PAGE_URL}"
                    );
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

/// Downloads the best matching release artifact into an isolated staging directory,
/// verifies its SHA-256 digest against `SHA256SUMS.txt`, extracts the payload safely,
/// and validates the staged `bypath-gui.exe` PE header.
pub async fn download_and_stage_update<F>(
    report: &UpdateCheckReport,
    config: Option<&Config>,
    has_local_windivert: bool,
    staging_parent: &Path,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Result<StagedUpdate>
where
    F: Fn(UpdateProgressEvent) + Send + Sync + 'static,
{
    let asset = select_update_asset(report, has_local_windivert)
        .ok_or_else(|| anyhow!("no compatible Windows x64 update asset found in release"))?;

    let checksums_url = report
        .checksums_asset_url
        .as_deref()
        .filter(|u| is_safe_release_url(u))
        .ok_or_else(|| {
            anyhow!(
                "release {} does not publish SHA256SUMS.txt; automatic installation refused",
                report.latest_tag
            )
        })?;

    if cancel.load(AtomicOrdering::SeqCst) {
        bail!("UPDATE_CANCELLED");
    }

    let safe_tag: String = report
        .latest_tag
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let staging_root = staging_parent.join(format!("stage-{safe_tag}-{}", now_epoch_ms()));
    fs::create_dir_all(&staging_root).with_context(|| {
        format!(
            "failed to create update staging directory '{}'",
            staging_root.display()
        )
    })?;

    let result = download_and_stage_inner(
        report,
        &asset,
        checksums_url,
        config,
        &staging_root,
        cancel,
        on_progress,
    )
    .await;

    if result.is_err() {
        let _ = fs::remove_dir_all(&staging_root);
    }
    result
}

async fn download_and_stage_inner<F>(
    report: &UpdateCheckReport,
    asset: &SelectedUpdateAsset,
    checksums_url: &str,
    config: Option<&Config>,
    staging_root: &Path,
    cancel: Arc<AtomicBool>,
    on_progress: F,
) -> Result<StagedUpdate>
where
    F: Fn(UpdateProgressEvent) + Send + Sync + 'static,
{
    let routes = build_fetch_routes(config, None, Some(&report.checked_via));

    // Step 1: Fetch SHA256SUMS.txt via the fastest working route.
    let mut checksums_text: Option<(String, String)> = None;
    let mut checksum_errors = Vec::new();
    for route in &routes {
        if cancel.load(AtomicOrdering::SeqCst) {
            bail!("UPDATE_CANCELLED");
        }
        match fetch_text_via_route(checksums_url, None, route, Duration::from_secs(20)).await {
            FetchOutcome::Ok(body) => {
                checksums_text = Some((body, route.label.clone()));
                break;
            }
            FetchOutcome::HttpError(code, _) => {
                checksum_errors.push(format!("{}: HTTP {code}", route.label));
            }
            FetchOutcome::TransportError(msg) => {
                checksum_errors.push(format!("{}: {msg}", route.label));
            }
        }
    }

    let (sums_body, sums_route) = checksums_text.ok_or_else(|| {
        anyhow!(
            "failed to download SHA256SUMS.txt ({})",
            checksum_errors.join("; ")
        )
    })?;
    let sums = parse_sha256sums(&sums_body);
    let expected_sha256 = sums
        .get(&asset.name.to_ascii_lowercase())
        .cloned()
        .ok_or_else(|| {
            anyhow!(
                "SHA256SUMS.txt does not contain an entry for '{}'",
                asset.name
            )
        })?;

    // Prioritize whichever route just succeeded for SHA256SUMS.txt.
    let download_routes = build_fetch_routes(config, None, Some(&sums_route));
    let download_part = staging_root.join(format!("{}.part", asset.name));
    let download_final = staging_root.join(&asset.name);

    let mut downloaded_route: Option<String> = None;
    let mut download_errors = Vec::new();

    for route in &download_routes {
        if cancel.load(AtomicOrdering::SeqCst) {
            bail!("UPDATE_CANCELLED");
        }
        on_progress(UpdateProgressEvent {
            stage: "downloading".into(),
            version: report.latest_version.clone(),
            tag: report.latest_tag.clone(),
            asset_name: Some(asset.name.clone()),
            downloaded_bytes: 0,
            total_bytes: asset.size,
            percent: 0,
            route: Some(route.label.clone()),
            error: None,
        });

        let route_label = route.label.clone();
        let report_ver = report.latest_version.clone();
        let report_tag = report.latest_tag.clone();
        let asset_name = asset.name.clone();

        match download_file_via_route(
            &asset.url,
            &download_part,
            asset.size,
            route,
            Duration::from_secs(600),
            &cancel,
            &|done, total| {
                on_progress(UpdateProgressEvent {
                    stage: "downloading".into(),
                    version: report_ver.clone(),
                    tag: report_tag.clone(),
                    asset_name: Some(asset_name.clone()),
                    downloaded_bytes: done,
                    total_bytes: total,
                    percent: compute_percent(done, total),
                    route: Some(route_label.clone()),
                    error: None,
                });
            },
        )
        .await
        {
            Ok(()) => {
                downloaded_route = Some(route.label.clone());
                break;
            }
            Err(err) if err.to_string().contains("UPDATE_CANCELLED") => {
                return Err(err);
            }
            Err(err) => {
                download_errors.push(format!("{}: {err}", route.label));
            }
        }
    }

    let via_route = downloaded_route.ok_or_else(|| {
        anyhow!(
            "failed to download update artifact via all routes ({})",
            download_errors.join("; ")
        )
    })?;

    if cancel.load(AtomicOrdering::SeqCst) {
        bail!("UPDATE_CANCELLED");
    }

    fs::rename(&download_part, &download_final)
        .context("failed to finalize downloaded update file")?;
    let actual_size = fs::metadata(&download_final).map(|m| m.len()).ok();

    // Step 2: Verify SHA-256 digest strictly on a blocking worker thread.
    on_progress(UpdateProgressEvent {
        stage: "verifying".into(),
        version: report.latest_version.clone(),
        tag: report.latest_tag.clone(),
        asset_name: Some(asset.name.clone()),
        downloaded_bytes: actual_size.unwrap_or(0),
        total_bytes: actual_size.or(asset.size),
        percent: 100,
        route: Some(via_route.clone()),
        error: None,
    });

    let verify_path = download_final.clone();
    let expected_clone = expected_sha256.clone();
    let verified_sha256 =
        tokio::task::spawn_blocking(move || verify_file_sha256(&verify_path, &expected_clone))
            .await
            .context("SHA-256 verification task panicked")??;

    if cancel.load(AtomicOrdering::SeqCst) {
        bail!("UPDATE_CANCELLED");
    }

    // Step 3: Extract / stage payload and verify PE binary header.
    on_progress(UpdateProgressEvent {
        stage: "extracting".into(),
        version: report.latest_version.clone(),
        tag: report.latest_tag.clone(),
        asset_name: Some(asset.name.clone()),
        downloaded_bytes: actual_size.unwrap_or(0),
        total_bytes: actual_size.or(asset.size),
        percent: 100,
        route: Some(via_route.clone()),
        error: None,
    });

    let extract_root = staging_root.join("payload");
    let asset_kind = asset.kind;
    let artifact_path = download_final.clone();
    let payload_dir = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let dir = match asset_kind {
            UpdateAssetKind::ZipBundle => extract_zip_safely(&artifact_path, &extract_root)?,
            UpdateAssetKind::StandaloneGuiExe => {
                fs::create_dir_all(&extract_root)?;
                let dst = extract_root.join("bypath-gui.exe");
                fs::copy(&artifact_path, &dst)?;
                extract_root
            }
        };
        verify_pe_executable(&dir.join("bypath-gui.exe"), MIN_GUI_EXE_BYTES)?;
        if dir.join("bypath-daemon.exe").is_file() {
            verify_pe_executable(&dir.join("bypath-daemon.exe"), MIN_GUI_EXE_BYTES)?;
        }
        if dir.join("bypath.exe").is_file() {
            verify_pe_executable(&dir.join("bypath.exe"), MIN_GUI_EXE_BYTES)?;
        }
        Ok(dir)
    })
    .await
    .context("update extraction task panicked")??;

    // Remove the original archive once the payload is safely extracted and verified.
    let _ = fs::remove_file(&download_final);

    on_progress(UpdateProgressEvent {
        stage: "ready".into(),
        version: report.latest_version.clone(),
        tag: report.latest_tag.clone(),
        asset_name: Some(asset.name.clone()),
        downloaded_bytes: actual_size.unwrap_or(0),
        total_bytes: actual_size.or(asset.size),
        percent: 100,
        route: Some(via_route.clone()),
        error: None,
    });

    Ok(StagedUpdate {
        target_version: report.latest_version.clone(),
        target_tag: report.latest_tag.clone(),
        asset_name: asset.name.clone(),
        asset_kind: asset.kind,
        staging_root: staging_root.to_path_buf(),
        payload_dir,
        sha256: verified_sha256,
        downloaded_via: via_route,
        staged_at_ms: now_epoch_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bypath-update-test-{label}-{}-{}",
            std::process::id(),
            now_epoch_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

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
        assert!(!is_safe_release_url(
            "http://github.com/Nexusyyan/bypath/releases"
        ));
        assert!(!is_safe_release_url(
            "https://github.com/Nexusyyan/bypath-evil/releases"
        ));
        assert!(!is_safe_release_url(
            "https://github.com/Nexusyyan/bypath/releases\ncalc.exe"
        ));
    }

    #[test]
    fn parses_github_release_payload_and_selects_assets() {
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
                    "name": "bypath-gui-v0.5.0-windows-x64.exe",
                    "browser_download_url": "https://github.com/Nexusyyan/bypath/releases/download/v0.5.0/bypath-gui-v0.5.0-windows-x64.exe",
                    "size": 4200000
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
        assert_eq!(report.nodriver_asset_size, Some(8_100_000));
        assert_eq!(
            report.gui_exe_asset_name.as_deref(),
            Some("bypath-gui-v0.5.0-windows-x64.exe")
        );
        assert!(report.checksums_asset_url.is_some());

        let with_driver = select_update_asset(&report, true).unwrap();
        assert_eq!(with_driver.name, "bypath-v0.5.0-windows-x64-nodriver.zip");
        assert_eq!(with_driver.kind, UpdateAssetKind::ZipBundle);

        let without_driver = select_update_asset(&report, false).unwrap();
        assert_eq!(without_driver.name, "bypath-v0.5.0-windows-x64.zip");
        assert_eq!(without_driver.kind, UpdateAssetKind::ZipBundle);
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
                },
                {
                    "name": "SHA256SUMS.txt",
                    "browser_download_url": "https://evil.example.com/SHA256SUMS.txt",
                    "size": 100
                }
            ]
        }"#;

        let report = parse_release_json("0.4.1", json, "direct").unwrap();
        assert!(!report.update_available);
        assert_eq!(report.release_url, RELEASES_PAGE_URL);
        assert!(report.primary_asset_url.is_none());
        assert!(report.checksums_asset_url.is_none());
    }

    #[test]
    fn parses_sha256sums_and_verifies_file_digest() {
        let dir = make_temp_dir("sha256");
        let file_path = dir.join("payload.bin");
        fs::write(&file_path, b"bypath-release-artifact-content").unwrap();
        let digest = compute_file_sha256(&file_path).unwrap();
        assert_eq!(digest.len(), 64);

        let sums_txt = format!(
            "# comment line\n{}  bypath-v0.5.0-windows-x64.zip\n{} *payload.bin\nbadhash  ignored.zip\n",
            "A".repeat(64),
            digest.to_ascii_uppercase()
        );
        let parsed = parse_sha256sums(&sums_txt);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed.get("payload.bin").map(String::as_str), Some(digest.as_str()));

        assert!(verify_file_sha256(&file_path, &digest).is_ok());
        assert!(verify_file_sha256(&file_path, &"0".repeat(64)).is_err());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extracts_valid_zip_and_rejects_zip_slip() {
        let dir = make_temp_dir("zip");
        let good_zip = dir.join("good.zip");
        {
            let f = File::create(&good_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default();
            zw.start_file("bypath-gui.exe", opts).unwrap();
            zw.write_all(b"MZfake-pe-header").unwrap();
            zw.start_file("skills/bypath-agent/SKILL.md", opts).unwrap();
            zw.write_all(b"# skill").unwrap();
            zw.finish().unwrap();
        }

        let extract_dir = dir.join("out");
        let payload = extract_zip_safely(&good_zip, &extract_dir).unwrap();
        assert_eq!(payload, extract_dir);
        assert!(verify_pe_executable(&payload.join("bypath-gui.exe"), 8).is_ok());
        assert!(payload.join("skills/bypath-agent/SKILL.md").is_file());

        let evil_zip = dir.join("evil.zip");
        {
            let f = File::create(&evil_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = SimpleFileOptions::default();
            zw.start_file("../escape.exe", opts).unwrap();
            zw.write_all(b"MZevil").unwrap();
            zw.finish().unwrap();
        }
        let evil_out = dir.join("evil-out");
        assert!(extract_zip_safely(&evil_zip, &evil_out).is_err());
        assert!(!dir.join("escape.exe").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
