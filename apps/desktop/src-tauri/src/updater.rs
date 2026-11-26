use anyhow::{anyhow, bail, Context, Result};
use bypath_daemon::{query_service_status, start_service, stop_service};
use bypath_ipc::{IpcClient, IpcRequest, IpcResponse};
use bypath_windivert::is_elevated;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const UPDATER_TEMP_SUBDIR: &str = "bypath-updater";
const OUTCOME_FILE_NAME: &str = "last-update-result.json";
const LOG_FILE_NAME: &str = "update.log";
const OLD_SUFFIX: &str = ".bypath-old";
const NEW_SUFFIX: &str = ".bypath-new";

const BUNDLE_EXTRA_FILES: &[&str] = &[
    "skills/bypath-agent/SKILL.md",
    "config.example.toml",
    "README.md",
    "README.zh-CN.md",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "THIRD_PARTY_NOTICES.md",
    "scripts/fetch-windivert.ps1",
];

const BUNDLE_WINDIVERT_MISSING_ONLY: &[&str] = &[
    "runtime/windivert/WinDivert.dll",
    "runtime/windivert/WinDivert64.sys",
    "runtime/windivert/WINDIVERT_LICENSE.txt",
    "runtime/windivert/WINDIVERT_README.txt",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyUpdatePlan {
    pub parent_pid: u32,
    pub from_version: String,
    pub target_version: String,
    pub target_tag: String,
    pub install_dir: PathBuf,
    pub current_gui_exe: PathBuf,
    pub staging_payload_dir: PathBuf,
    pub staging_root_to_clean: PathBuf,
    pub daemon_target_exe: Option<PathBuf>,
    pub cli_target_exe: Option<PathBuf>,
    pub update_bundle_extras: bool,
    pub service_was_running: bool,
    pub service_engine_was_running: bool,
    pub restore_foreground: bool,
    pub relaunch_unelevated: bool,
    pub result_file: PathBuf,
    pub log_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOutcomeRecord {
    pub ok: bool,
    pub from_version: String,
    pub target_version: String,
    pub target_tag: String,
    pub error: Option<String>,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone)]
struct FileSwapTask {
    src: PathBuf,
    dst: PathBuf,
    critical: bool,
    only_if_missing: bool,
}

#[derive(Debug, Clone)]
struct SwappedRecord {
    dst: PathBuf,
    backup: Option<PathBuf>,
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn updater_temp_root() -> PathBuf {
    std::env::temp_dir().join(UPDATER_TEMP_SUBDIR)
}

pub fn updater_state_dir() -> PathBuf {
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        return PathBuf::from(local).join("Bypath");
    }
    std::env::temp_dir().join("Bypath")
}

pub fn outcome_file_path() -> PathBuf {
    updater_state_dir().join(OUTCOME_FILE_NAME)
}

pub fn log_file_path() -> PathBuf {
    updater_state_dir().join(LOG_FILE_NAME)
}

pub fn take_last_update_outcome() -> Option<UpdateOutcomeRecord> {
    let path = outcome_file_path();
    let raw = fs::read_to_string(&path).ok()?;
    let _ = fs::remove_file(&path);
    serde_json::from_str::<UpdateOutcomeRecord>(&raw).ok()
}

pub fn cleanup_stale_updater_artifacts(extra_dirs: &[PathBuf]) {
    let temp_root = updater_temp_root();
    if let Ok(entries) = fs::read_dir(&temp_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                let _ = fs::remove_file(&path);
            }
        }
    }

    for dir in extra_dirs {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.ends_with(OLD_SUFFIX) || name.ends_with(NEW_SUFFIX) {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }
    }
}

pub fn is_bundle_install_dir(install_dir: &Path) -> bool {
    install_dir.join("bypath-daemon.exe").is_file()
        || install_dir.join("bypath.exe").is_file()
        || install_dir.join("config.example.toml").is_file()
        || install_dir.join("skills").is_dir()
}

pub fn is_dir_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(
        ".bypath-write-probe-{}-{}",
        std::process::id(),
        now_epoch_ms()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(f) => {
            drop(f);
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn plan_needs_elevation(plan: &ApplyUpdatePlan) -> bool {
    if plan.service_was_running {
        return true;
    }
    if !is_dir_writable(&plan.install_dir) {
        return true;
    }
    if let Some(ref daemon_exe) = plan.daemon_target_exe {
        if let Some(parent) = daemon_exe.parent() {
            if parent != plan.install_dir && !is_dir_writable(parent) {
                return true;
            }
        }
    }
    if let Some(ref cli_exe) = plan.cli_target_exe {
        if let Some(parent) = cli_exe.parent() {
            if parent != plan.install_dir && !is_dir_writable(parent) {
                return true;
            }
        }
    }
    false
}

/// Persists `plan` and spawns an isolated copy of the current executable in `%TEMP%\bypath-updater\`
/// with `--apply-update --plan <plan.json>`. Returns `Err("ELEVATION_CANCELLED")` if UAC was refused.
pub fn spawn_updater_helper(plan: &ApplyUpdatePlan) -> Result<(), String> {
    let temp_root = updater_temp_root();
    fs::create_dir_all(&temp_root)
        .map_err(|e| format!("failed to create updater temp directory: {e}"))?;

    let stamp = format!("{}-{}", plan.parent_pid, now_epoch_ms());
    let helper_exe = temp_root.join(format!("bypath-updater-{stamp}.exe"));
    let plan_path = temp_root.join(format!("plan-{stamp}.json"));

    fs::copy(&plan.current_gui_exe, &helper_exe)
        .map_err(|e| format!("failed to copy updater helper binary: {e}"))?;

    let plan_json = serde_json::to_string_pretty(plan)
        .map_err(|e| format!("failed to serialize update plan: {e}"))?;
    fs::write(&plan_path, plan_json)
        .map_err(|e| format!("failed to write update plan file: {e}"))?;

    let needs_uac = plan_needs_elevation(plan) && !is_elevated();

    #[cfg(windows)]
    {
        if needs_uac {
            if let Err(err) = spawn_elevated_helper(&helper_exe, &plan_path) {
                let _ = fs::remove_file(&helper_exe);
                let _ = fs::remove_file(&plan_path);
                return Err(err);
            }
            return Ok(());
        }

        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new(&helper_exe)
            .args(["--apply-update", "--plan"])
            .arg(&plan_path)
            .current_dir(&temp_root)
            .creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| {
                let _ = fs::remove_file(&helper_exe);
                let _ = fs::remove_file(&plan_path);
                format!("failed to spawn updater helper: {e}")
            })?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = needs_uac;
        let _ = fs::remove_file(&helper_exe);
        let _ = fs::remove_file(&plan_path);
        Err("in-app self-update is only implemented on Windows".into())
    }
}

/// Entrypoint invoked by `main.rs` before Tauri initializes when started with `--apply-update --plan <path>`.
pub fn maybe_run_updater_mode(args: &[String]) -> Option<i32> {
    if !args.iter().any(|a| a == "--apply-update") {
        return None;
    }
    let mut plan_path: Option<PathBuf> = None;
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == "--plan" && i + 1 < args.len() {
            plan_path = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else {
            i += 1;
        }
    }

    let Some(path) = plan_path else {
        return Some(2);
    };

    match run_apply_update_from_plan(&path) {
        Ok(()) => Some(0),
        Err(_) => Some(1),
    }
}

fn append_log(log_file: &Path, msg: &str) {
    if let Some(parent) = log_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(log_file) {
        let _ = writeln!(f, "[{}] {msg}", now_epoch_ms());
    }
}

fn write_outcome(plan: &ApplyUpdatePlan, ok: bool, error: Option<String>) {
    if let Some(parent) = plan.result_file.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let rec = UpdateOutcomeRecord {
        ok,
        from_version: plan.from_version.clone(),
        target_version: plan.target_version.clone(),
        target_tag: plan.target_tag.clone(),
        error,
        timestamp_ms: now_epoch_ms(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&rec) {
        let _ = fs::write(&plan.result_file, json);
    }
}

fn run_apply_update_from_plan(plan_path: &Path) -> Result<()> {
    let raw = fs::read_to_string(plan_path)
        .with_context(|| format!("failed to read update plan '{}'", plan_path.display()))?;
    let plan: ApplyUpdatePlan =
        serde_json::from_str(&raw).context("failed to parse update plan JSON")?;
    let _ = fs::remove_file(plan_path);

    append_log(
        &plan.log_file,
        &format!(
            "starting in-app update {} -> {} (parent_pid={}, service_was_running={}, restore_foreground={})",
            plan.from_version,
            plan.target_version,
            plan.parent_pid,
            plan.service_was_running,
            plan.restore_foreground
        ),
    );

    wait_for_parent_exit(plan.parent_pid, Duration::from_secs(20));
    append_log(&plan.log_file, "parent GUI process exited");

    let mut stopped_service = false;
    if plan.service_was_running {
        append_log(&plan.log_file, "stopping BypathService before binary swap");
        match stop_service_and_wait(Duration::from_secs(15)) {
            Ok(()) => {
                stopped_service = true;
                append_log(&plan.log_file, "BypathService stopped cleanly");
            }
            Err(err) => {
                let msg = format!("failed to stop BypathService before update: {err}");
                append_log(&plan.log_file, &msg);
                write_outcome(&plan, false, Some(msg.clone()));
                relaunch_gui(&plan, false);
                bail!(msg);
            }
        }
    }

    match apply_staged_files(&plan) {
        Ok(swapped_count) => {
            append_log(
                &plan.log_file,
                &format!("applied {swapped_count} staged file(s) successfully"),
            );
            let _ = fs::remove_dir_all(&plan.staging_root_to_clean);

            if stopped_service {
                append_log(&plan.log_file, "restarting BypathService after update");
                if let Err(err) = restart_service_and_engine(plan.service_engine_was_running) {
                    append_log(
                        &plan.log_file,
                        &format!("warning: BypathService recovery note: {err}"),
                    );
                } else {
                    append_log(&plan.log_file, "BypathService restarted and ready");
                }
            }

            write_outcome(&plan, true, None);
            relaunch_gui(&plan, true);
            append_log(&plan.log_file, "relaunched updated Bypath GUI; helper done");
            Ok(())
        }
        Err(err) => {
            let msg = format!("in-app update failed and rolled back: {err}");
            append_log(&plan.log_file, &msg);
            if stopped_service {
                let _ = restart_service_and_engine(plan.service_engine_was_running);
            }
            write_outcome(&plan, false, Some(msg.clone()));
            relaunch_gui(&plan, false);
            Err(anyhow!(msg))
        }
    }
}

fn build_swap_tasks(plan: &ApplyUpdatePlan) -> Result<Vec<FileSwapTask>> {
    let mut tasks = Vec::new();

    let gui_src = plan.staging_payload_dir.join("bypath-gui.exe");
    if !gui_src.is_file() {
        bail!("staged payload is missing bypath-gui.exe");
    }
    tasks.push(FileSwapTask {
        src: gui_src,
        dst: plan.current_gui_exe.clone(),
        critical: true,
        only_if_missing: false,
    });

    if let Some(ref daemon_dst) = plan.daemon_target_exe {
        if !same_path_lossy(daemon_dst, &plan.current_gui_exe) {
            let daemon_src = plan.staging_payload_dir.join("bypath-daemon.exe");
            if daemon_src.is_file() {
                tasks.push(FileSwapTask {
                    src: daemon_src,
                    dst: daemon_dst.clone(),
                    critical: true,
                    only_if_missing: false,
                });
            }
        }
    }

    if let Some(ref cli_dst) = plan.cli_target_exe {
        let cli_src = plan.staging_payload_dir.join("bypath.exe");
        if cli_src.is_file() {
            tasks.push(FileSwapTask {
                src: cli_src,
                dst: cli_dst.clone(),
                critical: true,
                only_if_missing: false,
            });
        }
    }

    if plan.update_bundle_extras {
        for rel in BUNDLE_EXTRA_FILES {
            let src = plan.staging_payload_dir.join(rel);
            if src.is_file() {
                tasks.push(FileSwapTask {
                    src,
                    dst: plan.install_dir.join(rel),
                    critical: false,
                    only_if_missing: false,
                });
            }
        }
        for rel in BUNDLE_WINDIVERT_MISSING_ONLY {
            let src = plan.staging_payload_dir.join(rel);
            if src.is_file() {
                tasks.push(FileSwapTask {
                    src,
                    dst: plan.install_dir.join(rel),
                    critical: false,
                    only_if_missing: true,
                });
            }
        }
    }

    Ok(tasks)
}

fn same_path_lossy(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

fn suffixed_path(path: &Path, suffix: &str) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push(suffix);
    PathBuf::from(os)
}

fn retry_fs<F>(attempts: usize, delay: Duration, mut op: F) -> io_result::IoResult<()>
where
    F: FnMut() -> io_result::IoResult<()>,
{
    let mut last_err = None;
    for attempt in 0..attempts {
        match op() {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_err = Some(err);
                if attempt + 1 < attempts {
                    std::thread::sleep(delay);
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| std::io::Error::other("filesystem operation failed")))
}

mod io_result {
    pub type IoResult<T> = std::io::Result<T>;
}

/// Applies all staged files transactionally with full automatic rollback on any critical failure.
pub fn apply_staged_files(plan: &ApplyUpdatePlan) -> Result<usize> {
    apply_staged_files_with_retry(plan, 25, Duration::from_millis(150))
}

fn apply_staged_files_with_retry(
    plan: &ApplyUpdatePlan,
    attempts: usize,
    delay: Duration,
) -> Result<usize> {
    let tasks = build_swap_tasks(plan)?;
    let mut applied: Vec<SwappedRecord> = Vec::new();
    let mut replaced_count = 0usize;

    for task in tasks {
        if task.only_if_missing && task.dst.exists() {
            continue;
        }
        match swap_single_file(&task.src, &task.dst, attempts, delay) {
            Ok(record) => {
                applied.push(record);
                replaced_count += 1;
            }
            Err(err) => {
                if task.critical {
                    rollback_swapped_files(&applied);
                    return Err(err).with_context(|| {
                        format!("failed to replace '{}'", task.dst.display())
                    });
                }
                append_log(
                    &plan.log_file,
                    &format!(
                        "non-critical file '{}' skipped due to error: {err}",
                        task.dst.display()
                    ),
                );
            }
        }
    }

    // Best-effort cleanup of `.bypath-old` backups after every critical file succeeded.
    for rec in &applied {
        if let Some(ref bak) = rec.backup {
            let _ = fs::remove_file(bak);
        }
    }

    Ok(replaced_count)
}

fn swap_single_file(
    src: &Path,
    dst: &Path,
    attempts: usize,
    delay: Duration,
) -> Result<SwappedRecord> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp_new = suffixed_path(dst, NEW_SUFFIX);
    let backup = suffixed_path(dst, OLD_SUFFIX);
    let _ = fs::remove_file(&tmp_new);
    fs::copy(src, &tmp_new)
        .with_context(|| format!("failed to copy staged file to '{}'", tmp_new.display()))?;

    let had_existing = dst.exists();
    if had_existing {
        let _ = fs::remove_file(&backup);
        if let Err(err) = retry_fs(attempts, delay, || fs::rename(dst, &backup)) {
            let _ = fs::remove_file(&tmp_new);
            return Err(err).with_context(|| {
                format!(
                    "file '{}' is locked and could not be backed up",
                    dst.display()
                )
            });
        }
    }

    if let Err(err) = retry_fs(attempts, delay, || fs::rename(&tmp_new, dst)) {
        let _ = fs::remove_file(&tmp_new);
        if had_existing && backup.exists() {
            let _ = retry_fs(10, Duration::from_millis(50), || fs::rename(&backup, dst));
        }
        return Err(err).with_context(|| {
            format!("failed to move staged binary into '{}'", dst.display())
        });
    }

    Ok(SwappedRecord {
        dst: dst.to_path_buf(),
        backup: if had_existing { Some(backup) } else { None },
    })
}

fn rollback_swapped_files(applied: &[SwappedRecord]) {
    for rec in applied.iter().rev() {
        let _ = fs::remove_file(&rec.dst);
        if let Some(ref backup) = rec.backup {
            let _ = retry_fs(15, Duration::from_millis(100), || {
                fs::rename(backup, &rec.dst)
            });
        }
    }
}

fn stop_service_and_wait(timeout: Duration) -> Result<()> {
    let st = query_service_status();
    if !st.installed || (!st.running && st.state == "stopped") {
        return Ok(());
    }
    stop_service()?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let cur = query_service_status();
        if !cur.running && cur.state == "stopped" {
            // Brief grace period so Windows releases the image section handle.
            std::thread::sleep(Duration::from_millis(300));
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    bail!("timed out waiting for BypathService to stop")
}

fn restart_service_and_engine(start_engine: bool) -> Result<()> {
    start_service()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let ipc = IpcClient::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ready = false;
        while Instant::now() < deadline {
            if matches!(ipc.request(&IpcRequest::Ping).await, Ok(IpcResponse::Pong { .. })) {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(180)).await;
        }
        if !ready {
            bail!("BypathService started, but IPC pipe did not become ready within 10s");
        }
        if start_engine {
            let _ = ipc.request(&IpcRequest::StartEngine).await;
        }
        Ok(())
    })
}

fn wait_for_parent_exit(pid: u32, timeout: Duration) {
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };
        unsafe {
            if let Ok(handle) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                let ms = timeout.as_millis().min(u32::MAX as u128) as u32;
                let _ = WaitForSingleObject(handle, ms);
                let _ = CloseHandle(handle);
                std::thread::sleep(Duration::from_millis(180));
                return;
            }
        }
    }
    let _ = timeout;
    std::thread::sleep(Duration::from_millis(400));
}

fn relaunch_gui(plan: &ApplyUpdatePlan, update_succeeded: bool) {
    let mut args: Vec<&str> = Vec::new();
    if update_succeeded && plan.restore_foreground {
        args.push("--start-foreground");
    }
    args.push("--handoff");

    #[cfg(windows)]
    {
        if plan.relaunch_unelevated
            && !(update_succeeded && plan.restore_foreground)
            && is_elevated()
            && spawn_unelevated_via_shell(&plan.current_gui_exe, &args, &plan.install_dir).is_ok()
        {
            return;
        }
    }

    let _ = Command::new(&plan.current_gui_exe)
        .args(&args)
        .current_dir(&plan.install_dir)
        .spawn();
}

#[cfg(windows)]
fn to_wide_null(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn spawn_elevated_helper(helper_exe: &Path, plan_path: &Path) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_CANCELLED};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;

    let verb = to_wide_null("runas");
    let file = to_wide_null(&helper_exe.display().to_string());
    let params = to_wide_null(&format!(
        "--apply-update --plan \"{}\"",
        plan_path.display()
    ));
    let work_dir = helper_exe
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
            SW_HIDE,
        )
    };

    let code = res.0 as isize;
    if code <= 32 {
        let last_err = unsafe { GetLastError() };
        if last_err == ERROR_CANCELLED || code == 5 {
            return Err("ELEVATION_CANCELLED".into());
        }
        return Err(format!("failed to launch elevated updater helper (code {code})"));
    }
    Ok(())
}

/// Attempts to relaunch the updated GUI using the desktop shell (`explorer.exe`) primary token
/// so standard non-admin users return to an unelevated WebView2 session after an elevated service update.
#[cfg(windows)]
fn spawn_unelevated_via_shell(exe_path: &Path, args: &[&str], work_dir: &Path) -> Result<()> {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        DuplicateTokenEx, SecurityImpersonation, TokenPrimary, TOKEN_ALL_ACCESS,
        TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{
        CreateProcessWithTokenW, OpenProcess, OpenProcessToken, CREATE_UNICODE_ENVIRONMENT,
        LOGON_WITH_PROFILE, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, STARTUPINFOW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};

    let shell_hwnd = unsafe { GetShellWindow() };
    if shell_hwnd.0.is_null() {
        bail!("desktop shell window unavailable");
    }
    let mut shell_pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(shell_hwnd, Some(&mut shell_pid));
    }
    if shell_pid == 0 {
        bail!("desktop shell PID unavailable");
    }

    let shell_proc = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, shell_pid) }
        .context("failed to open shell process")?;
    let mut shell_token = HANDLE::default();
    let open_tok = unsafe {
        OpenProcessToken(
            shell_proc,
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut shell_token,
        )
    };
    unsafe {
        let _ = CloseHandle(shell_proc);
    }
    open_tok.context("failed to open shell process token")?;

    let mut primary_token = HANDLE::default();
    let dup_res = unsafe {
        DuplicateTokenEx(
            shell_token,
            TOKEN_ALL_ACCESS,
            None,
            SecurityImpersonation,
            TokenPrimary,
            &mut primary_token,
        )
    };
    unsafe {
        let _ = CloseHandle(shell_token);
    }
    dup_res.context("failed to duplicate shell primary token")?;

    let mut cmdline = format!("\"{}\"", exe_path.display());
    for arg in args {
        cmdline.push(' ');
        cmdline.push_str(arg);
    }
    let mut cmdline_w = to_wide_null(&cmdline);
    let work_dir_w = to_wide_null(&work_dir.display().to_string());

    let si = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    let mut pi = PROCESS_INFORMATION::default();
    let create_res = unsafe {
        CreateProcessWithTokenW(
            primary_token,
            LOGON_WITH_PROFILE,
            PCWSTR::null(),
            PWSTR(cmdline_w.as_mut_ptr()),
            CREATE_UNICODE_ENVIRONMENT,
            None,
            PCWSTR(work_dir_w.as_ptr()),
            &si,
            &mut pi,
        )
    };
    unsafe {
        let _ = CloseHandle(primary_token);
    }
    create_res.context("CreateProcessWithTokenW failed")?;
    unsafe {
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(pi.hThread);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bypath-updater-test-{label}-{}-{}",
            std::process::id(),
            now_epoch_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn applies_bundle_update_while_preserving_user_data_and_existing_windivert() {
        let root = temp_workspace("apply-ok");
        let install = root.join("install");
        let staging = root.join("staging");
        let payload = staging.join("payload");
        fs::create_dir_all(install.join("runtime/windivert")).unwrap();
        fs::create_dir_all(install.join("bypath-data")).unwrap();
        fs::create_dir_all(payload.join("runtime/windivert")).unwrap();
        fs::create_dir_all(payload.join("skills/bypath-agent")).unwrap();

        // Existing installation files
        fs::write(install.join("bypath-gui.exe"), b"MZ-old-gui").unwrap();
        fs::write(install.join("bypath-daemon.exe"), b"MZ-old-daemon").unwrap();
        fs::write(install.join("bypath.exe"), b"MZ-old-cli").unwrap();
        fs::write(install.join("runtime/windivert/WinDivert.dll"), b"existing-driver").unwrap();
        fs::write(install.join("portable.json"), b"{}").unwrap();
        fs::write(install.join("bypath-data/config.toml"), b"user-config").unwrap();

        // Staged update payload
        fs::write(payload.join("bypath-gui.exe"), b"MZ-new-gui-0.5.0").unwrap();
        fs::write(payload.join("bypath-daemon.exe"), b"MZ-new-daemon-0.5.0").unwrap();
        fs::write(payload.join("bypath.exe"), b"MZ-new-cli-0.5.0").unwrap();
        fs::write(payload.join("skills/bypath-agent/SKILL.md"), b"# updated skill").unwrap();
        fs::write(payload.join("runtime/windivert/WinDivert.dll"), b"staged-driver").unwrap();
        fs::write(
            payload.join("runtime/windivert/WinDivert64.sys"),
            b"staged-sys-missing-locally",
        )
        .unwrap();

        let plan = ApplyUpdatePlan {
            parent_pid: 0,
            from_version: "0.4.2".into(),
            target_version: "0.5.0".into(),
            target_tag: "v0.5.0".into(),
            install_dir: install.clone(),
            current_gui_exe: install.join("bypath-gui.exe"),
            staging_payload_dir: payload,
            staging_root_to_clean: staging,
            daemon_target_exe: Some(install.join("bypath-daemon.exe")),
            cli_target_exe: Some(install.join("bypath.exe")),
            update_bundle_extras: true,
            service_was_running: false,
            service_engine_was_running: false,
            restore_foreground: false,
            relaunch_unelevated: false,
            result_file: root.join("result.json"),
            log_file: root.join("update.log"),
        };

        let count = apply_staged_files(&plan).unwrap();
        assert!(count >= 4);
        assert_eq!(fs::read(install.join("bypath-gui.exe")).unwrap(), b"MZ-new-gui-0.5.0");
        assert_eq!(
            fs::read(install.join("bypath-daemon.exe")).unwrap(),
            b"MZ-new-daemon-0.5.0"
        );
        assert_eq!(fs::read(install.join("bypath.exe")).unwrap(), b"MZ-new-cli-0.5.0");
        assert_eq!(
            fs::read(install.join("skills/bypath-agent/SKILL.md")).unwrap(),
            b"# updated skill"
        );
        // Existing WinDivert.dll must stay untouched; missing WinDivert64.sys gets populated.
        assert_eq!(
            fs::read(install.join("runtime/windivert/WinDivert.dll")).unwrap(),
            b"existing-driver"
        );
        assert_eq!(
            fs::read(install.join("runtime/windivert/WinDivert64.sys")).unwrap(),
            b"staged-sys-missing-locally"
        );
        // User portable files must never be modified.
        assert_eq!(fs::read(install.join("portable.json")).unwrap(), b"{}");
        assert_eq!(
            fs::read(install.join("bypath-data/config.toml")).unwrap(),
            b"user-config"
        );
        // Backups should be cleaned up automatically.
        assert!(!suffixed_path(&install.join("bypath-gui.exe"), OLD_SUFFIX).exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rolls_back_earlier_files_when_critical_swap_fails() {
        let root = temp_workspace("rollback");
        let install = root.join("install");
        let staging = root.join("staging");
        let payload = staging.join("payload");
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&payload).unwrap();

        fs::write(install.join("bypath-gui.exe"), b"MZ-old-gui").unwrap();
        fs::write(install.join("bypath-daemon.exe"), b"MZ-old-daemon").unwrap();
        // Place a non-empty directory at the backup path so backing up bypath-daemon.exe fails deterministically.
        let blocker_dir = suffixed_path(&install.join("bypath-daemon.exe"), OLD_SUFFIX);
        fs::create_dir_all(&blocker_dir).unwrap();
        fs::write(blocker_dir.join("lock.txt"), b"blocker").unwrap();

        fs::write(payload.join("bypath-gui.exe"), b"MZ-new-gui").unwrap();
        fs::write(payload.join("bypath-daemon.exe"), b"MZ-new-daemon").unwrap();

        let plan = ApplyUpdatePlan {
            parent_pid: 0,
            from_version: "0.4.2".into(),
            target_version: "0.5.0".into(),
            target_tag: "v0.5.0".into(),
            install_dir: install.clone(),
            current_gui_exe: install.join("bypath-gui.exe"),
            staging_payload_dir: payload,
            staging_root_to_clean: staging,
            daemon_target_exe: Some(install.join("bypath-daemon.exe")),
            cli_target_exe: None,
            update_bundle_extras: false,
            service_was_running: false,
            service_engine_was_running: false,
            restore_foreground: false,
            relaunch_unelevated: false,
            result_file: root.join("result.json"),
            log_file: root.join("update.log"),
        };

        assert!(apply_staged_files_with_retry(&plan, 2, Duration::from_millis(10)).is_err());
        // Original bypath-gui.exe and bypath-daemon.exe must be restored cleanly after rollback.
        assert_eq!(fs::read(install.join("bypath-gui.exe")).unwrap(), b"MZ-old-gui");
        assert_eq!(
            fs::read(install.join("bypath-daemon.exe")).unwrap(),
            b"MZ-old-daemon"
        );

        let _ = fs::remove_dir_all(&root);
    }
}
