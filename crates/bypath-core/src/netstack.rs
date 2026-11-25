use crate::capture::{parse_ip_packet, CaptureEngine, FiveTuple, PacketMeta, TransportProtocol};
use crate::config::ProxyConfig;
use crate::engine::RuntimeConfig;
use crate::process::ProcessTracker;
use crate::rules::DomainDecision;
use crate::sniff::{sniff_tcp_payload, SniffResult};
use crate::telemetry::{
    ConnectionAction, ConnectionProtocol, NewConnectionParams, TelemetryHub,
};
use crate::upstream::{dial_tcp_via_proxy, TargetAddr};
use bytes::Bytes;
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{
    Checksum, ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken,
};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer as TcpSocketBuffer, State as TcpState};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address, Ipv6Address};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};

const TCP_RX_BUF_SIZE: usize = 65_536;
const TCP_TX_BUF_SIZE: usize = 65_536;
const CHANNEL_CAPACITY: usize = 64;
const SNIFF_PEEK_TIMEOUT: Duration = Duration::from_millis(150);

/// Packet delivered from the dispatcher to the userspace TCP netstack.
pub struct NetstackInput {
    pub packet: Vec<u8>,
    pub tuple: FiveTuple,
    pub meta: PacketMeta,
    pub proxy: Option<ProxyConfig>,
    pub proxy_name: Option<String>,
    pub rule_name: String,
    pub pid: Option<u32>,
    pub process_name: String,
    pub exe_path: Option<String>,
    pub process_label: String,
    pub is_syn_only: bool,
}

/// Event sent back from a per-connection upstream relay task into the netstack loop.
enum UpstreamEvent {
    Data(Bytes),
    Eof,
    Abort,
}

struct VirtualRxToken(Vec<u8>);

impl RxToken for VirtualRxToken {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

struct VirtualTxToken<'a>(&'a mut VecDeque<Vec<u8>>);

impl<'a> TxToken for VirtualTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let res = f(&mut buf);
        self.0.push_back(buf);
        res
    }
}

struct VirtualIpDevice {
    rx_queue: VecDeque<Vec<u8>>,
    tx_queue: VecDeque<Vec<u8>>,
    mtu: usize,
}

impl VirtualIpDevice {
    fn new(mtu: usize) -> Self {
        Self {
            rx_queue: VecDeque::with_capacity(256),
            tx_queue: VecDeque::with_capacity(256),
            mtu,
        }
    }
}

impl Device for VirtualIpDevice {
    type RxToken<'a> = VirtualRxToken;
    type TxToken<'a> = VirtualTxToken<'a>;

    fn receive(
        &mut self,
        _timestamp: SmolInstant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let pkt = self.rx_queue.pop_front()?;
        Some((VirtualRxToken(pkt), VirtualTxToken(&mut self.tx_queue)))
    }

    fn transmit(&mut self, _timestamp: SmolInstant) -> Option<Self::TxToken<'_>> {
        Some(VirtualTxToken(&mut self.tx_queue))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        // Windows NIC checksum offloading often delivers outbound packets with zeroed or
        // partial checksums. Ignore Rx checksum checks while still generating valid Tx checksums.
        let mut csum = ChecksumCapabilities::ignored();
        csum.ipv4 = Checksum::Tx;
        csum.tcp = Checksum::Tx;
        csum.udp = Checksum::Tx;
        caps.checksum = csum;
        caps
    }
}

struct ActiveTcpSession {
    handle: SocketHandle,
    to_upstream: Option<mpsc::Sender<Bytes>>,
    from_upstream: mpsc::Receiver<UpstreamEvent>,
    pending_write: VecDeque<u8>,
    upstream_eof: bool,
    upstream_abort: bool,
    created_at: Instant,
}

/// Userspace TCP/IP stack backed by `smoltcp` with `any_ip` enabled.
pub struct TcpNetstack {
    input_tx: mpsc::Sender<NetstackInput>,
}

impl TcpNetstack {
    pub(crate) fn spawn(
        capture: Arc<dyn CaptureEngine>,
        tracker: Arc<dyn ProcessTracker>,
        runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
        enable_sniffing: Arc<AtomicBool>,
        telemetry: Arc<TelemetryHub>,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (input_tx, input_rx) = mpsc::channel::<NetstackInput>(1024);
        let handle = tokio::spawn(run_netstack_loop(
            capture,
            tracker,
            runtime,
            input_rx,
            enable_sniffing,
            telemetry,
        ));
        (Self { input_tx }, handle)
    }

    pub async fn feed(&self, input: NetstackInput) -> Result<(), &'static str> {
        self.input_tx
            .send(input)
            .await
            .map_err(|_| "netstack channel closed")
    }

    pub fn try_feed(&self, input: NetstackInput) -> Result<(), &'static str> {
        self.input_tx.try_send(input).map_err(|err| match err {
            mpsc::error::TrySendError::Full(_) => "netstack channel full",
            mpsc::error::TrySendError::Closed(_) => "netstack channel closed",
        })
    }
}

async fn run_netstack_loop(
    capture: Arc<dyn CaptureEngine>,
    tracker: Arc<dyn ProcessTracker>,
    runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
    mut input_rx: mpsc::Receiver<NetstackInput>,
    enable_sniffing: Arc<AtomicBool>,
    telemetry: Arc<TelemetryHub>,
) {
    let mut device = VirtualIpDevice::new(1500);
    let mut iface_cfg = IfaceConfig::new(HardwareAddress::Ip);
    iface_cfg.random_seed = 0xA99B_C0DE;

    let origin = Instant::now();
    let now_smol = |origin: Instant| -> SmolInstant {
        SmolInstant::from_millis(origin.elapsed().as_millis() as i64)
    };

    let mut iface = Interface::new(iface_cfg, &mut device, now_smol(origin));
    iface.set_any_ip(true);
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(10, 0, 0, 1), 0));
        let _ = addrs.push(IpCidr::new(
            IpAddress::v6(0xfd00, 0, 0, 0, 0, 0, 0, 1),
            0,
        ));
    });
    let _ = iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 0, 1));
    let _ = iface
        .routes_mut()
        .add_default_ipv6_route(Ipv6Address::new(0xfd00, 0, 0, 0, 0, 0, 0, 1));

    let mut sockets = SocketSet::new(vec![]);
    let mut sessions: HashMap<FiveTuple, ActiveTcpSession> = HashMap::new();
    let mut flow_metas: HashMap<FiveTuple, PacketMeta> = HashMap::new();
    let wake_notify = Arc::new(Notify::new());

    loop {
        let now = now_smol(origin);
        let delay = iface
            .poll_delay(now, &sockets)
            .map(|d| Duration::from_millis(d.total_millis().clamp(1, 25)))
            .unwrap_or(Duration::from_millis(25));

        tokio::select! {
            biased;
            maybe_input = input_rx.recv() => {
                match maybe_input {
                    Some(first_input) => {
                        let sniff = enable_sniffing.load(Ordering::Relaxed);
                        handle_input(
                            first_input,
                            &mut device,
                            &mut iface,
                            &mut sockets,
                            &mut sessions,
                            &mut flow_metas,
                            &wake_notify,
                            now_smol(origin),
                            sniff,
                            &telemetry,
                            &tracker,
                            &runtime,
                        );
                        // Batch-drain additional queued inputs without waiting
                        while let Ok(extra) = input_rx.try_recv() {
                            handle_input(
                                extra,
                                &mut device,
                                &mut iface,
                                &mut sockets,
                                &mut sessions,
                                &mut flow_metas,
                                &wake_notify,
                                now_smol(origin),
                                sniff,
                                &telemetry,
                                &tracker,
                                &runtime,
                            );
                        }
                    }
                    None => break,
                }
            }
            _ = wake_notify.notified() => {}
            _ = tokio::time::sleep(delay) => {}
        }

        // Poll stack and pump data between smoltcp sockets and upstream Tokio tasks
        let ts = now_smol(origin);
        iface.poll(ts, &mut device, &mut sockets);
        pump_sessions(&mut sockets, &mut sessions, &mut flow_metas);
        iface.poll(now_smol(origin), &mut device, &mut sockets);

        // Flush all generated reply IP packets back into the OS stack as inbound replies
        flush_tx_packets(&mut device, &flow_metas, capture.as_ref());
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_input(
    mut input: NetstackInput,
    device: &mut VirtualIpDevice,
    iface: &mut Interface,
    sockets: &mut SocketSet<'_>,
    sessions: &mut HashMap<FiveTuple, ActiveTcpSession>,
    flow_metas: &mut HashMap<FiveTuple, PacketMeta>,
    wake_notify: &Arc<Notify>,
    now: SmolInstant,
    enable_sniffing: bool,
    telemetry: &Arc<TelemetryHub>,
    tracker: &Arc<dyn ProcessTracker>,
    runtime: &Arc<RwLock<Arc<RuntimeConfig>>>,
) {
    normalize_offloaded_ip_lengths(&mut input.packet);
    flow_metas.insert(input.tuple, input.meta);

    // If a fresh SYN arrives reusing a 5-tuple whose previous session is already winding down,
    // immediately reap the old socket so the new connection succeeds without waiting for TIME_WAIT.
    if input.is_syn_only {
        let should_replace = sessions.get(&input.tuple).is_some_and(|existing| {
            let state = sockets.get::<TcpSocket>(existing.handle).state();
            matches!(
                state,
                TcpState::TimeWait | TcpState::Closed | TcpState::Closing | TcpState::LastAck
            )
        });
        if should_replace {
            if let Some(old) = sessions.remove(&input.tuple) {
                sockets.remove(old.handle);
            }
        }
    }

    if input.is_syn_only && !sessions.contains_key(&input.tuple) {
        let rx_buf = TcpSocketBuffer::new(vec![0u8; TCP_RX_BUF_SIZE]);
        let tx_buf = TcpSocketBuffer::new(vec![0u8; TCP_TX_BUF_SIZE]);
        let mut sock = TcpSocket::new(rx_buf, tx_buf);
        sock.set_nagle_enabled(false);
        sock.set_ack_delay(None);

        let listen_ep = IpEndpoint::new(to_smol_ip(input.tuple.dst_ip), input.tuple.dst_port);
        if let Err(err) = sock.listen(listen_ep) {
            warn!(?err, tuple = ?input.tuple, "smoltcp socket listen failed");
            return;
        }

        let handle = sockets.add(sock);
        let (to_up_tx, to_up_rx) = mpsc::channel::<Bytes>(CHANNEL_CAPACITY);
        let (from_up_tx, from_up_rx) = mpsc::channel::<UpstreamEvent>(CHANNEL_CAPACITY);

        sessions.insert(
            input.tuple,
            ActiveTcpSession {
                handle,
                to_upstream: Some(to_up_tx),
                from_upstream: from_up_rx,
                pending_write: VecDeque::new(),
                upstream_eof: false,
                upstream_abort: false,
                created_at: Instant::now(),
            },
        );

        let initial_action = if input.proxy.is_some() {
            ConnectionAction::Proxy
        } else {
            ConnectionAction::Direct
        };
        let conn_id = telemetry.open_connection(NewConnectionParams {
            pid: input.pid,
            process_name: input.process_name,
            exe_path: input.exe_path,
            protocol: ConnectionProtocol::Tcp,
            src_addr: SocketAddr::new(input.tuple.src_ip, input.tuple.src_port).to_string(),
            dst_ip: input.tuple.dst_ip.to_string(),
            dst_port: input.tuple.dst_port,
            rule_name: input.rule_name,
            proxy_name: input.proxy_name.clone(),
            action: initial_action,
        });

        let notify_clone = Arc::clone(wake_notify);
        let telemetry_clone = Arc::clone(telemetry);
        let tracker_clone = Arc::clone(tracker);
        let runtime_clone = Arc::clone(runtime);
        let tuple = input.tuple;
        let pid = input.pid;
        let proxy = input.proxy;
        let proxy_name = input.proxy_name;
        let process_label = input.process_label;

        tokio::spawn(async move {
            run_upstream_tcp_relay(
                conn_id,
                pid,
                tuple,
                proxy,
                proxy_name,
                process_label,
                to_up_rx,
                from_up_tx,
                notify_clone,
                enable_sniffing,
                telemetry_clone,
                tracker_clone,
                runtime_clone,
            )
            .await;
        });

        // Immediately feed this SYN and poll once so this listening socket transitions
        // to SynReceived bound to `src_ip:src_port` before any concurrent SYN to the
        // same `dst_ip:dst_port` is processed.
        device.rx_queue.push_back(input.packet);
        iface.poll(now, device, sockets);
        return;
    }

    if sessions.contains_key(&input.tuple) {
        device.rx_queue.push_back(input.packet);
    }
}

fn pump_sessions(
    sockets: &mut SocketSet<'_>,
    sessions: &mut HashMap<FiveTuple, ActiveTcpSession>,
    flow_metas: &mut HashMap<FiveTuple, PacketMeta>,
) {
    let mut to_remove = Vec::new();

    for (tuple, sess) in sessions.iter_mut() {
        let sock = sockets.get_mut::<TcpSocket>(sess.handle);

        // 1. Drain upstream events into pending write queue
        while let Ok(evt) = sess.from_upstream.try_recv() {
            match evt {
                UpstreamEvent::Data(bytes) => {
                    sess.pending_write.extend(bytes.iter().copied());
                }
                UpstreamEvent::Eof => {
                    sess.upstream_eof = true;
                }
                UpstreamEvent::Abort => {
                    sess.upstream_abort = true;
                }
            }
        }

        if sess.upstream_abort {
            sock.abort();
            to_remove.push(*tuple);
            continue;
        }

        // 2. App -> Upstream (respect bounded channel backpressure!)
        if sock.can_recv() {
            if let Some(ref tx) = sess.to_upstream {
                if let Ok(permit) = tx.try_reserve() {
                    let mut tmp = vec![0u8; 16_384];
                    match sock.recv_slice(&mut tmp) {
                        Ok(n) if n > 0 => {
                            tmp.truncate(n);
                            permit.send(Bytes::from(tmp));
                        }
                        _ => {}
                    }
                }
            }
        } else if matches!(
            sock.state(),
            TcpState::CloseWait
                | TcpState::LastAck
                | TcpState::Closing
                | TcpState::TimeWait
                | TcpState::Closed
        ) && sess.to_upstream.is_some()
        {
            // App actually sent FIN (never trigger during Listen / SynReceived!)
            sess.to_upstream = None;
        }

        // 3. Upstream -> App
        if sock.can_send() && !sess.pending_write.is_empty() {
            let (first, _) = sess.pending_write.as_slices();
            if !first.is_empty() {
                if let Ok(sent) = sock.send_slice(first) {
                    if sent > 0 {
                        sess.pending_write.drain(..sent);
                    }
                }
            }
            if sock.can_send() && !sess.pending_write.is_empty() {
                let (first, _) = sess.pending_write.as_slices();
                if !first.is_empty() {
                    if let Ok(sent) = sock.send_slice(first) {
                        if sent > 0 {
                            sess.pending_write.drain(..sent);
                        }
                    }
                }
            }
        }

        // 4. Propagate clean upstream EOF once all buffered data is handed to smoltcp
        if sess.upstream_eof && sess.pending_write.is_empty() && sock.may_send() {
            sock.close();
        }

        // 5. Reap closed or stale handshake sockets
        let stale_handshake =
            matches!(sock.state(), TcpState::Listen | TcpState::SynReceived)
                && sess.created_at.elapsed() > Duration::from_secs(15);

        if !sock.is_active() || sock.state() == TcpState::Closed || stale_handshake {
            if stale_handshake {
                sock.abort();
            }
            to_remove.push(*tuple);
        }
    }

    for tuple in to_remove {
        if let Some(sess) = sessions.remove(&tuple) {
            sockets.remove(sess.handle);
        }
        flow_metas.remove(&tuple);
    }
}

fn flush_tx_packets(
    device: &mut VirtualIpDevice,
    flow_metas: &HashMap<FiveTuple, PacketMeta>,
    capture: &dyn CaptureEngine,
) {
    while let Some(pkt) = device.tx_queue.pop_front() {
        let Some(parsed) = parse_ip_packet(&pkt) else {
            continue;
        };
        if parsed.tuple.protocol != TransportProtocol::Tcp {
            continue;
        }
        // Reply packet has src=remote, dst=local; look up original outbound tuple (reversed)
        let orig_tuple = parsed.tuple.reversed();
        if let Some(meta) = flow_metas.get(&orig_tuple) {
            let reply_meta = meta.as_inbound_reply();
            if let Err(err) = capture.inject(&pkt, &reply_meta) {
                debug!(?err, tuple = ?orig_tuple, "failed to inject TCP reply packet");
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_upstream_tcp_relay(
    conn_id: u64,
    pid: Option<u32>,
    tuple: FiveTuple,
    mut proxy: Option<ProxyConfig>,
    mut proxy_name: Option<String>,
    process_label: String,
    mut to_upstream_rx: mpsc::Receiver<Bytes>,
    from_upstream_tx: mpsc::Sender<UpstreamEvent>,
    wake_notify: Arc<Notify>,
    enable_sniffing: bool,
    telemetry: Arc<TelemetryHub>,
    tracker: Arc<dyn ProcessTracker>,
    runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
) {
    // Wait briefly for the first client payload chunk so we can sniff TLS SNI / HTTP Host
    // without blocking server-speaks-first protocols (SSH, SMTP, etc.). Always peek in
    // `InspectDirect` mode (`proxy.is_none()`) so domain rules remain enforceable.
    let should_peek = enable_sniffing || proxy.is_none();
    let first_chunk = if should_peek {
        match tokio::time::timeout(SNIFF_PEEK_TIMEOUT, to_upstream_rx.recv()).await {
            Ok(Some(bytes)) => Some(bytes),
            Ok(None) => {
                telemetry.close_connection(conn_id, None);
                return;
            }
            Err(_) => None,
        }
    } else {
        None
    };

    let (target, sniff_note, sniffed_host) = match first_chunk.as_deref().and_then(sniff_tcp_payload) {
        Some(SniffResult::TlsSni(host)) => {
            telemetry.update_sniff(conn_id, host.clone(), "SNI");
            (
                TargetAddr::Domain(host.clone(), tuple.dst_port),
                format!("SNI:{host}"),
                Some(host),
            )
        }
        Some(SniffResult::HttpHost(host)) => {
            telemetry.update_sniff(conn_id, host.clone(), "HTTP");
            (
                TargetAddr::Domain(host.clone(), tuple.dst_port),
                format!("HTTP:{host}"),
                Some(host),
            )
        }
        _ => (
            TargetAddr::Ip(SocketAddr::new(tuple.dst_ip, tuple.dst_port)),
            "IP".to_string(),
            None,
        ),
    };

    // Phase 2 domain-level routing evaluation (Block / Direct / Proxy override)
    if let Some(ref host) = sniffed_host {
        let rt = Arc::clone(&runtime.read().expect("runtime config lock poisoned"));
        if let Some(decision) = rt.rules.evaluate_domain(pid, &tuple, host, tracker.as_ref()) {
            match decision {
                DomainDecision::Block { rule_name } => {
                    info!(
                        proc = %process_label,
                        proto = "TCP",
                        host = %host,
                        port = tuple.dst_port,
                        rule = %rule_name,
                        "blocked connection by domain rule"
                    );
                    telemetry.mark_connection_blocked(
                        conn_id,
                        rule_name.clone(),
                        Some(format!("Blocked domain '{host}' by rule '{rule_name}'")),
                    );
                    let _ = from_upstream_tx.send(UpstreamEvent::Abort).await;
                    wake_notify.notify_one();
                    return;
                }
                DomainDecision::Direct { rule_name } => {
                    proxy = None;
                    proxy_name = None;
                    telemetry.update_connection_route(
                        conn_id,
                        ConnectionAction::Direct,
                        rule_name,
                        None,
                    );
                }
                DomainDecision::Proxy {
                    rule_name,
                    proxy_name: override_proxy_name,
                } => {
                    if let Some(override_cfg) = rt.proxies.get(&override_proxy_name).cloned() {
                        proxy = Some(override_cfg);
                        proxy_name = Some(override_proxy_name.clone());
                        telemetry.update_connection_route(
                            conn_id,
                            ConnectionAction::Proxy,
                            rule_name,
                            Some(override_proxy_name),
                        );
                    }
                }
            }
        }
    }

    let route_label = proxy_name.as_deref().unwrap_or("direct");
    info!(
        proc = %process_label,
        proto = "TCP",
        target = %target,
        sniff = %sniff_note,
        route = %route_label,
        "forwarding connection"
    );

    let stream = match proxy {
        Some(ref proxy_cfg) => dial_tcp_via_proxy(proxy_cfg, &target).await,
        None => {
            let direct_addr = SocketAddr::new(tuple.dst_ip, tuple.dst_port);
            match TcpStream::connect(direct_addr).await {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    Ok(s)
                }
                Err(err) => Err(anyhow::Error::from(err)),
            }
        }
    };

    let stream = match stream {
        Ok(s) => s,
        Err(err) => {
            let err_msg = err.to_string();
            warn!(
                proc = %process_label,
                target = %target,
                route = %route_label,
                error = %err_msg,
                "upstream connection dial failed"
            );
            telemetry.close_connection(conn_id, Some(err_msg));
            let _ = from_upstream_tx.send(UpstreamEvent::Abort).await;
            wake_notify.notify_one();
            return;
        }
    };

    let (mut up_read, mut up_write) = stream.into_split();

    if let Some(first) = first_chunk {
        let len = first.len() as u64;
        if let Err(err) = up_write.write_all(&first).await {
            debug!(?err, %target, "failed writing initial sniffed chunk to upstream");
            telemetry.close_connection(conn_id, Some(err.to_string()));
            let _ = from_upstream_tx.send(UpstreamEvent::Abort).await;
            wake_notify.notify_one();
            return;
        }
        telemetry.add_bytes(conn_id, len, 0);
    }

    // Pump local -> upstream
    let writer_telemetry = Arc::clone(&telemetry);
    let writer = tokio::spawn(async move {
        while let Some(chunk) = to_upstream_rx.recv().await {
            let len = chunk.len() as u64;
            if up_write.write_all(&chunk).await.is_err() {
                break;
            }
            writer_telemetry.add_bytes(conn_id, len, 0);
        }
        let _ = up_write.shutdown().await;
    });

    // Pump upstream -> local
    let mut buf = vec![0u8; 16_384];
    let mut close_error: Option<String> = None;
    loop {
        match up_read.read(&mut buf).await {
            Ok(0) => {
                let _ = from_upstream_tx.send(UpstreamEvent::Eof).await;
                wake_notify.notify_one();
                break;
            }
            Ok(n) => {
                telemetry.add_bytes(conn_id, 0, n as u64);
                let data = Bytes::copy_from_slice(&buf[..n]);
                if from_upstream_tx.send(UpstreamEvent::Data(data)).await.is_err() {
                    break;
                }
                wake_notify.notify_one();
            }
            Err(err) => {
                close_error = Some(err.to_string());
                let _ = from_upstream_tx.send(UpstreamEvent::Abort).await;
                wake_notify.notify_one();
                break;
            }
        }
    }

    let _ = writer.await;
    telemetry.close_connection(conn_id, close_error);
}

fn to_smol_ip(ip: IpAddr) -> IpAddress {
    match ip {
        IpAddr::V4(v4) => IpAddress::Ipv4(Ipv4Address::from_bytes(&v4.octets())),
        IpAddr::V6(v6) => IpAddress::Ipv6(Ipv6Address::from_bytes(&v6.octets())),
    }
}

/// Fixes zeroed IPv4 Total Length / IPv6 Payload Length fields emitted by Windows LSOv2/TSO NICs
/// so `smoltcp` parses offloaded outbound packets cleanly.
fn normalize_offloaded_ip_lengths(packet: &mut [u8]) {
    if packet.is_empty() {
        return;
    }
    match packet[0] >> 4 {
        4 if packet.len() >= 20
            && packet.len() <= u16::MAX as usize
            && u16::from_be_bytes([packet[2], packet[3]]) == 0 =>
        {
            let len_bytes = (packet.len() as u16).to_be_bytes();
            packet[2] = len_bytes[0];
            packet[3] = len_bytes[1];
        }
        6 if packet.len() >= 40
            && (packet.len() - 40) <= u16::MAX as usize
            && u16::from_be_bytes([packet[4], packet[5]]) == 0 =>
        {
            let payload_len = ((packet.len() - 40) as u16).to_be_bytes();
            packet[4] = payload_len[0];
            packet[5] = payload_len[1];
        }
        _ => {}
    }
}
