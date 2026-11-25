use crate::ffi::{
    decode_windivert_addr, WinDivertAddress, WinDivertDataNetwork, WINDIVERT_EVENT_FLOW_DELETED,
    WINDIVERT_EVENT_FLOW_ESTABLISHED, WINDIVERT_EVENT_SOCKET_BIND, WINDIVERT_EVENT_SOCKET_CLOSE,
    WINDIVERT_EVENT_SOCKET_CONNECT, WINDIVERT_FLAG_RECV_ONLY, WINDIVERT_FLAG_SNIFF,
    WINDIVERT_LAYER_FLOW, WINDIVERT_LAYER_NETWORK, WINDIVERT_LAYER_SOCKET,
    WINDIVERT_PARAM_QUEUE_LENGTH, WINDIVERT_PARAM_QUEUE_SIZE, WINDIVERT_PARAM_QUEUE_TIME,
    WINDIVERT_PARAM_VERSION_MAJOR, WINDIVERT_PARAM_VERSION_MINOR,
};
use crate::iphelper::{find_tcp_listener_pid, lookup_pid_from_os_table};
use crate::loader::{WinDivertHandle, WinDivertLib};
use anyhow::Result;
use bypath_core::capture::{
    CaptureEngine, CapturedPacket, FiveTuple, PacketMeta, TransportProtocol,
};
use bypath_core::process::ProcessTracker;
use dashmap::DashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// Diagnostic status for the local WinDivert driver stack.
#[derive(Debug, Clone)]
pub struct WinDivertProbeReport {
    pub dll_loaded: bool,
    pub dll_path: Option<String>,
    pub driver_opened: bool,
    pub driver_version: Option<(u64, u64)>,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

fn classify_windivert_error(dll_loaded: bool, msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("winDivert64.sys".to_ascii_lowercase().as_str())
        && (lower.contains("missing") || lower.contains("not found") || lower.contains("os error 2"))
    {
        return "missing_sys".to_string();
    }
    if !dll_loaded && lower.contains("windivert.dll not found") {
        return "missing_dll".to_string();
    }
    if lower.contains("os error 5") || lower.contains("access denied") {
        return "access_denied".to_string();
    }
    if lower.contains("os error 1275") || lower.contains("hvci") || lower.contains("antivirus") {
        return "blocked_by_security".to_string();
    }
    if lower.contains("os error 577") || lower.contains("signature") {
        return "invalid_signature".to_string();
    }
    if lower.contains("os error 2") {
        return "missing_sys".to_string();
    }
    "other".to_string()
}

/// Probes `WinDivert.dll` loading and verifies that `WinDivert64.sys` opens cleanly
/// with a harmless no-op filter (`"false"`).
pub fn probe_windivert(windivert_dir: Option<&Path>) -> WinDivertProbeReport {
    let lib = match WinDivertLib::load(windivert_dir) {
        Ok(l) => l,
        Err(err) => {
            let msg = err.to_string();
            let kind = classify_windivert_error(false, &msg);
            return WinDivertProbeReport {
                dll_loaded: false,
                dll_path: None,
                driver_opened: false,
                driver_version: None,
                error: Some(msg),
                error_kind: Some(kind),
            };
        }
    };

    let dll_path = Some(lib.dll_path().display().to_string());
    match lib.open(
        "false",
        WINDIVERT_LAYER_NETWORK,
        0,
        WINDIVERT_FLAG_SNIFF | WINDIVERT_FLAG_RECV_ONLY,
    ) {
        Ok(handle) => {
            let major = handle.get_param(WINDIVERT_PARAM_VERSION_MAJOR).ok();
            let minor = handle.get_param(WINDIVERT_PARAM_VERSION_MINOR).ok();
            let version = major.zip(minor);
            drop(handle);
            WinDivertProbeReport {
                dll_loaded: true,
                dll_path,
                driver_opened: true,
                driver_version: version,
                error: None,
                error_kind: None,
            }
        }
        Err(err) => {
            let msg = err.to_string();
            let kind = classify_windivert_error(true, &msg);
            WinDivertProbeReport {
                dll_loaded: true,
                dll_path,
                driver_opened: false,
                driver_version: None,
                error: Some(msg),
                error_kind: Some(kind),
            }
        }
    }
}

/// Checks whether the current process has Windows Administrator elevation.
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// Windows capture backend combining WinDivert's SOCKET/FLOW layers (native PID events)
/// with the NETWORK layer (raw outbound packet diversion & inbound reply injection).
pub struct WinDivertCaptureEngine {
    lib: Arc<WinDivertLib>,
    /// Exact 5-tuple -> PID map populated by SOCKET/FLOW layers.
    tuple_pids: Arc<DashMap<FiveTuple, u32>>,
    /// Fallback `(protocol, local_port) -> PID` map for wildcard-bound UDP/TCP sockets.
    port_pids: Arc<DashMap<(TransportProtocol, u16), u32>>,
    /// Optional process tracker warmed asynchronously whenever SOCKET/FLOW reports a new PID.
    process_tracker: Option<Arc<dyn ProcessTracker>>,
    net_handle: Arc<Mutex<Option<Arc<WinDivertHandle>>>>,
    sock_handle: Arc<Mutex<Option<Arc<WinDivertHandle>>>>,
    flow_handle: Arc<Mutex<Option<Arc<WinDivertHandle>>>>,
    running: Arc<AtomicBool>,
}

impl WinDivertCaptureEngine {
    pub fn new(windivert_dir: Option<&Path>) -> Result<Self> {
        Self::with_process_tracker(windivert_dir, None)
    }

    pub fn with_process_tracker(
        windivert_dir: Option<&Path>,
        process_tracker: Option<Arc<dyn ProcessTracker>>,
    ) -> Result<Self> {
        let lib = WinDivertLib::load(windivert_dir)?;
        info!(dll = %lib.dll_path().display(), "loaded WinDivert.dll dynamically");
        Ok(Self {
            lib,
            tuple_pids: Arc::new(DashMap::new()),
            port_pids: Arc::new(DashMap::new()),
            process_tracker,
            net_handle: Arc::new(Mutex::new(None)),
            sock_handle: Arc::new(Mutex::new(None)),
            flow_handle: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn lib(&self) -> &Arc<WinDivertLib> {
        &self.lib
    }
}

impl CaptureEngine for WinDivertCaptureEngine {
    fn start(&self, tx: mpsc::Sender<CapturedPacket>) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        // 1. Open SOCKET layer (sniff-only) for immediate socket connect/bind PID events
        let sock = Arc::new(self.lib.open(
            "tcp or udp",
            WINDIVERT_LAYER_SOCKET,
            100,
            WINDIVERT_FLAG_SNIFF | WINDIVERT_FLAG_RECV_ONLY,
        )?);

        // 2. Open FLOW layer (sniff-only) for full established/deleted 5-tuple <-> PID mapping
        let flow = Arc::new(self.lib.open(
            "outbound and !loopback and (tcp or udp)",
            WINDIVERT_LAYER_FLOW,
            90,
            WINDIVERT_FLAG_SNIFF | WINDIVERT_FLAG_RECV_ONLY,
        )?);

        // 3. Open NETWORK layer (divert outbound non-loopback TCP/UDP packets)
        let net = Arc::new(self.lib.open(
            "outbound and !loopback and (tcp or udp)",
            WINDIVERT_LAYER_NETWORK,
            0,
            0,
        )?);

        // Tune WinDivert kernel packet queues for smooth throughput under bursts
        let _ = net.set_param(WINDIVERT_PARAM_QUEUE_LENGTH, 8192);
        let _ = net.set_param(WINDIVERT_PARAM_QUEUE_TIME, 4000);
        let _ = net.set_param(WINDIVERT_PARAM_QUEUE_SIZE, 16 * 1024 * 1024);

        *self.sock_handle.lock().unwrap() = Some(Arc::clone(&sock));
        *self.flow_handle.lock().unwrap() = Some(Arc::clone(&flow));
        *self.net_handle.lock().unwrap() = Some(Arc::clone(&net));

        // Optional non-blocking PID cache warmer thread so newly spawned processes are
        // resolved before their first TCP SYN packet hits the NETWORK dispatcher.
        let warm_tx = self.process_tracker.as_ref().map(|tracker| {
            let (tx, rx) = std::sync::mpsc::sync_channel::<u32>(512);
            let tracker_clone = Arc::clone(tracker);
            let running_clone = Arc::clone(&self.running);
            let _ = thread::Builder::new()
                .name("windivert-pid-warmer".into())
                .spawn(move || {
                    while running_clone.load(Ordering::Relaxed) {
                        match rx.recv_timeout(std::time::Duration::from_millis(250)) {
                            Ok(pid) => tracker_clone.warm_pid(pid),
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    }
                });
            tx
        });

        // Thread 1: SOCKET event listener
        {
            let running = Arc::clone(&self.running);
            let tuple_pids = Arc::clone(&self.tuple_pids);
            let port_pids = Arc::clone(&self.port_pids);
            let warm_tx = warm_tx.clone();
            thread::Builder::new()
                .name("windivert-socket".into())
                .spawn(move || {
                    let mut empty = [];
                    while running.load(Ordering::Relaxed) {
                        match sock.recv(&mut empty) {
                            Ok((_, addr)) => {
                                handle_socket_or_flow_event(
                                    &addr,
                                    &tuple_pids,
                                    &port_pids,
                                    warm_tx.as_ref(),
                                );
                            }
                            Err(err) => {
                                if running.load(Ordering::Relaxed) {
                                    debug!(?err, "WinDivert SOCKET recv ended");
                                }
                                break;
                            }
                        }
                    }
                })?;
        }

        // Thread 2: FLOW event listener
        {
            let running = Arc::clone(&self.running);
            let tuple_pids = Arc::clone(&self.tuple_pids);
            let port_pids = Arc::clone(&self.port_pids);
            let warm_tx = warm_tx.clone();
            thread::Builder::new()
                .name("windivert-flow".into())
                .spawn(move || {
                    let mut empty = [];
                    while running.load(Ordering::Relaxed) {
                        match flow.recv(&mut empty) {
                            Ok((_, addr)) => {
                                handle_socket_or_flow_event(
                                    &addr,
                                    &tuple_pids,
                                    &port_pids,
                                    warm_tx.as_ref(),
                                );
                            }
                            Err(err) => {
                                if running.load(Ordering::Relaxed) {
                                    debug!(?err, "WinDivert FLOW recv ended");
                                }
                                break;
                            }
                        }
                    }
                })?;
        }

        // Thread 3: NETWORK raw packet capture loop
        {
            let running = Arc::clone(&self.running);
            thread::Builder::new()
                .name("windivert-network".into())
                .spawn(move || {
                    let mut buf = vec![0u8; 65_535];
                    while running.load(Ordering::Relaxed) {
                        match net.recv(&mut buf) {
                            Ok((n, addr)) if n > 0 => {
                                let net_data = addr.network();
                                let captured = CapturedPacket {
                                    payload: buf[..n].to_vec(),
                                    meta: PacketMeta {
                                        iface_index: net_data.if_idx,
                                        sub_iface_index: net_data.sub_if_idx,
                                        outbound: addr.outbound(),
                                        is_ipv6: addr.ipv6(),
                                    },
                                };
                                if tx.blocking_send(captured).is_err() {
                                    break;
                                }
                            }
                            Ok(_) => {}
                            Err(err) => {
                                if running.load(Ordering::Relaxed) {
                                    warn!(?err, "WinDivert NETWORK recv error");
                                }
                                break;
                            }
                        }
                    }
                })?;
        }

        Ok(())
    }

    fn inject(&self, packet: &[u8], meta: &PacketMeta) -> Result<()> {
        let guard = self.net_handle.lock().unwrap();
        let Some(ref handle) = *guard else {
            return Ok(());
        };
        let mut owned = packet.to_vec();
        let mut addr = WinDivertAddress::default();
        addr.set_layer(WINDIVERT_LAYER_NETWORK as u8);
        addr.set_outbound(meta.outbound);
        addr.set_ipv6(meta.is_ipv6);
        addr.set_network(WinDivertDataNetwork {
            if_idx: meta.iface_index,
            sub_if_idx: meta.sub_iface_index,
        });
        self.lib.calc_checksums(&mut owned, &mut addr);
        handle.send(&owned, &addr)?;
        Ok(())
    }

    fn lookup_pid(&self, tuple: &FiveTuple) -> Option<u32> {
        if let Some(pid) = self.tuple_pids.get(tuple) {
            return Some(*pid);
        }
        if let Some(pid) = lookup_pid_from_os_table(tuple) {
            self.tuple_pids.insert(*tuple, pid);
            return Some(pid);
        }
        if let Some(pid) = self.port_pids.get(&(tuple.protocol, tuple.src_port)) {
            return Some(*pid);
        }
        None
    }

    fn find_local_tcp_listener_pid(&self, port: u16) -> Option<u32> {
        find_tcp_listener_pid(port)
    }

    fn shutdown(&self) -> Result<()> {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.net_handle.lock().unwrap().take() {
            h.shutdown();
        }
        if let Some(h) = self.sock_handle.lock().unwrap().take() {
            h.shutdown();
        }
        if let Some(h) = self.flow_handle.lock().unwrap().take() {
            h.shutdown();
        }
        Ok(())
    }
}

fn handle_socket_or_flow_event(
    addr: &WinDivertAddress,
    tuple_pids: &DashMap<FiveTuple, u32>,
    port_pids: &DashMap<(TransportProtocol, u16), u32>,
    warm_tx: Option<&std::sync::mpsc::SyncSender<u32>>,
) {
    let data = addr.socket();
    let proto = match data.protocol {
        6 => TransportProtocol::Tcp,
        17 => TransportProtocol::Udp,
        _ => return,
    };

    let is_ipv6 = addr.ipv6();
    let src_ip = decode_windivert_addr(data.local_addr, is_ipv6);
    let dst_ip = decode_windivert_addr(data.remote_addr, is_ipv6);
    let tuple = FiveTuple {
        protocol: proto,
        src_ip,
        src_port: data.local_port,
        dst_ip,
        dst_port: data.remote_port,
    };

    match addr.event() {
        WINDIVERT_EVENT_FLOW_ESTABLISHED | WINDIVERT_EVENT_SOCKET_CONNECT => {
            if data.process_id != 0 {
                tuple_pids.insert(tuple, data.process_id);
                if data.local_port != 0 {
                    port_pids.insert((proto, data.local_port), data.process_id);
                }
                if let Some(tx) = warm_tx {
                    let _ = tx.try_send(data.process_id);
                }
            }
        }
        WINDIVERT_EVENT_SOCKET_BIND => {
            if data.process_id != 0 && data.local_port != 0 {
                port_pids.insert((proto, data.local_port), data.process_id);
                if let Some(tx) = warm_tx {
                    let _ = tx.try_send(data.process_id);
                }
            }
        }
        WINDIVERT_EVENT_FLOW_DELETED => {
            tuple_pids.remove(&tuple);
        }
        WINDIVERT_EVENT_SOCKET_CLOSE => {
            tuple_pids.remove(&tuple);
            if data.local_port != 0 {
                port_pids.remove(&(proto, data.local_port));
            }
        }
        _ => {}
    }
}
