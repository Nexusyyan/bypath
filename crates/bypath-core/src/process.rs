use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::RwLock;
use std::time::{Duration, Instant};

/// Metadata describing a running OS process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent_pid: u32,
    pub name: String,
    pub exe_path: Option<String>,
}

/// Platform-agnostic process & ancestor-chain tracker.
pub trait ProcessTracker: Send + Sync + 'static {
    fn get_process(&self, pid: u32) -> Option<ProcessInfo>;

    /// Returns `[self, parent, grandparent, ...]` up to root or cycle boundary.
    fn ancestor_chain(&self, pid: u32) -> Vec<ProcessInfo>;

    /// Returns a snapshot of all currently visible processes.
    fn list_processes(&self) -> Vec<ProcessInfo>;

    /// Pre-warms cache metadata for a newly observed PID (e.g., from WinDivert SOCKET events).
    fn warm_pid(&self, pid: u32) {
        let _ = self.get_process(pid);
    }

    /// Forces a refresh of the underlying cache.
    fn refresh(&self);
}

struct CachedSnapshot {
    fetched_at: Instant,
    by_pid: HashMap<u32, ProcessInfo>,
}

/// Snapshot-backed process tracker with TTL caching and instant miss-triggered refresh.
pub struct SnapshotProcessTracker {
    ttl: Duration,
    cache: RwLock<CachedSnapshot>,
}

impl SnapshotProcessTracker {
    pub fn new(ttl: Duration) -> Self {
        let initial = take_os_process_snapshot();
        Self {
            ttl,
            cache: RwLock::new(CachedSnapshot {
                fetched_at: Instant::now(),
                by_pid: initial,
            }),
        }
    }

    fn ensure_fresh(&self, force: bool) {
        if !force {
            if let Ok(guard) = self.cache.read() {
                if guard.fetched_at.elapsed() < self.ttl {
                    return;
                }
            }
        }
        let fresh = take_os_process_snapshot();
        if let Ok(mut guard) = self.cache.write() {
            guard.by_pid = fresh;
            guard.fetched_at = Instant::now();
        }
    }
}

impl Default for SnapshotProcessTracker {
    fn default() -> Self {
        Self::new(Duration::from_millis(500))
    }
}

impl ProcessTracker for SnapshotProcessTracker {
    fn get_process(&self, pid: u32) -> Option<ProcessInfo> {
        if pid == 0 {
            return None;
        }
        self.ensure_fresh(false);
        if let Ok(guard) = self.cache.read() {
            if let Some(info) = guard.by_pid.get(&pid) {
                return Some(info.clone());
            }
        }
        // Fast O(1) per-PID lookup for newly spawned short-lived child processes before
        // paying the cost of a full system Toolhelp32 snapshot.
        if let Some(info) = query_single_process(pid) {
            if let Ok(mut guard) = self.cache.write() {
                guard.by_pid.insert(pid, info.clone());
            }
            return Some(info);
        }
        self.ensure_fresh(true);
        self.cache
            .read()
            .ok()
            .and_then(|g| g.by_pid.get(&pid).cloned())
    }

    fn ancestor_chain(&self, pid: u32) -> Vec<ProcessInfo> {
        if pid == 0 {
            return Vec::new();
        }
        self.ensure_fresh(false);

        // Opportunistically resolve any missing PID + immediate ancestors in O(depth).
        let mut probe = pid;
        let mut visited_probe = HashSet::new();
        while probe > 4 && visited_probe.insert(probe) && visited_probe.len() < 16 {
            let cached_ppid = self
                .cache
                .read()
                .ok()
                .and_then(|g| g.by_pid.get(&probe).map(|p| p.parent_pid));
            match cached_ppid {
                Some(ppid) => probe = ppid,
                None => {
                    if let Some(info) = query_single_process(probe) {
                        let next_ppid = info.parent_pid;
                        if let Ok(mut guard) = self.cache.write() {
                            guard.by_pid.insert(probe, info);
                        }
                        probe = next_ppid;
                    } else {
                        self.ensure_fresh(true);
                        break;
                    }
                }
            }
        }

        let guard = match self.cache.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        let mut chain = Vec::with_capacity(6);
        let mut visited = HashSet::new();
        let mut current = pid;

        while current != 0 && visited.insert(current) && chain.len() < 32 {
            match guard.by_pid.get(&current) {
                Some(info) => {
                    let next = info.parent_pid;
                    chain.push(info.clone());
                    current = next;
                }
                None => break,
            }
        }
        chain
    }

    fn warm_pid(&self, pid: u32) {
        if pid <= 4 {
            return;
        }
        let already_cached = self
            .cache
            .read()
            .ok()
            .map(|g| g.by_pid.contains_key(&pid))
            .unwrap_or(false);
        if !already_cached {
            let _ = self.ancestor_chain(pid);
        }
    }

    fn list_processes(&self) -> Vec<ProcessInfo> {
        self.ensure_fresh(true);
        let mut list: Vec<ProcessInfo> = self
            .cache
            .read()
            .ok()
            .map(|g| g.by_pid.values().cloned().collect())
            .unwrap_or_default();
        list.sort_by_key(|p| p.pid);
        list
    }

    fn refresh(&self) {
        self.ensure_fresh(true);
    }
}

/// In-memory mock tracker for deterministic unit tests.
#[derive(Default)]
pub struct MockProcessTracker {
    by_pid: RwLock<HashMap<u32, ProcessInfo>>,
}

impl MockProcessTracker {
    pub fn with_processes(procs: impl IntoIterator<Item = ProcessInfo>) -> Self {
        let map = procs.into_iter().map(|p| (p.pid, p)).collect();
        Self {
            by_pid: RwLock::new(map),
        }
    }
}

impl ProcessTracker for MockProcessTracker {
    fn get_process(&self, pid: u32) -> Option<ProcessInfo> {
        self.by_pid.read().ok()?.get(&pid).cloned()
    }

    fn ancestor_chain(&self, pid: u32) -> Vec<ProcessInfo> {
        let guard = match self.by_pid.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        let mut current = pid;
        while current != 0 && visited.insert(current) && chain.len() < 32 {
            match guard.get(&current) {
                Some(info) => {
                    let next = info.parent_pid;
                    chain.push(info.clone());
                    current = next;
                }
                None => break,
            }
        }
        chain
    }

    fn list_processes(&self) -> Vec<ProcessInfo> {
        self.by_pid
            .read()
            .ok()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    fn refresh(&self) {}
}

#[cfg(windows)]
fn query_single_process(pid: u32) -> Option<ProcessInfo> {
    use std::ffi::c_void;
    use std::path::Path;
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    #[repr(C)]
    struct ProcessBasicInformation {
        exit_status: i32,
        peb_base_address: *mut c_void,
        affinity_mask: usize,
        base_priority: i32,
        unique_process_id: usize,
        inherited_from_unique_process_id: usize,
    }

    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            process_handle: *mut c_void,
            process_information_class: u32,
            process_information: *mut c_void,
            process_information_length: u32,
            return_length: *mut u32,
        ) -> i32;
    }

    if pid <= 4 {
        return None;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut buf = vec![0u16; 1024];
        let mut size = buf.len() as u32;
        let exe_path = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut size,
        )
        .ok()
        .map(|()| String::from_utf16_lossy(&buf[..size as usize]));

        let mut pbi = std::mem::zeroed::<ProcessBasicInformation>();
        let mut ret_len = 0u32;
        let status = NtQueryInformationProcess(
            handle.0,
            0, // ProcessBasicInformation
            (&mut pbi as *mut ProcessBasicInformation).cast(),
            std::mem::size_of::<ProcessBasicInformation>() as u32,
            &mut ret_len,
        );
        let _ = CloseHandle(handle);

        let parent_pid = if status >= 0 {
            pbi.inherited_from_unique_process_id as u32
        } else {
            0
        };

        let name = exe_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())?;

        Some(ProcessInfo {
            pid,
            parent_pid,
            name,
            exe_path,
        })
    }
}

#[cfg(not(windows))]
fn query_single_process(_pid: u32) -> Option<ProcessInfo> {
    None
}

#[cfg(windows)]
fn take_os_process_snapshot() -> HashMap<u32, ProcessInfo> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    let mut map = HashMap::new();
    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return map,
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let pid = entry.th32ProcessID;
                let parent_pid = entry.th32ParentProcessID;
                let name_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);

                let exe_path = if pid > 4 {
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                        .ok()
                        .and_then(|handle| {
                            let mut buf = vec![0u16; 1024];
                            let mut size = buf.len() as u32;
                            let res = QueryFullProcessImageNameW(
                                handle,
                                PROCESS_NAME_WIN32,
                                PWSTR(buf.as_mut_ptr()),
                                &mut size,
                            );
                            let _ = CloseHandle(handle);
                            res.ok()
                                .map(|()| String::from_utf16_lossy(&buf[..size as usize]))
                        })
                } else {
                    None
                };

                map.insert(
                    pid,
                    ProcessInfo {
                        pid,
                        parent_pid,
                        name,
                        exe_path,
                    },
                );

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
    }
    map
}

#[cfg(not(windows))]
fn take_os_process_snapshot() -> HashMap<u32, ProcessInfo> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_ancestor_chain_walks_grandchild_to_root() {
        let tracker = MockProcessTracker::with_processes([
            ProcessInfo {
                pid: 100,
                parent_pid: 4,
                name: "Antigravity.exe".to_string(),
                exe_path: Some(r"C:\Program Files\Antigravity\Antigravity.exe".to_string()),
            },
            ProcessInfo {
                pid: 200,
                parent_pid: 100,
                name: "language_server_windows_x64.exe".to_string(),
                exe_path: Some(
                    r"C:\Program Files\Antigravity\resources\language_server_windows_x64.exe"
                        .to_string(),
                ),
            },
            ProcessInfo {
                pid: 300,
                parent_pid: 200,
                name: "node.exe".to_string(),
                exe_path: Some(r"C:\Program Files\nodejs\node.exe".to_string()),
            },
        ]);

        let chain = tracker.ancestor_chain(300);
        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].name, "node.exe");
        assert_eq!(chain[1].name, "language_server_windows_x64.exe");
        assert_eq!(chain[2].name, "Antigravity.exe");
    }

    #[cfg(windows)]
    #[test]
    fn fast_single_pid_lookup_resolves_current_process() {
        let info = query_single_process(std::process::id()).expect("current process info");
        assert_eq!(info.pid, std::process::id());
        assert!(info.parent_pid > 0);
        assert!(!info.name.is_empty());
        assert!(info.exe_path.is_some());
    }
}
