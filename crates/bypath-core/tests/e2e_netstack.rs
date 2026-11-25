//! End-to-end integration tests exercising `ProxyEngine`, `smoltcp` userspace TCP stack,
//! TLS SNI / HTTP Host domain recovery, SOCKS5 TCP & UDP ASSOCIATE, HTTP CONNECT,
//! process-tree inheritance, and direct passthrough for non-matching processes.

use anyhow::Result;
use bypath_core::capture::{
    build_udp_ip_packet, parse_ip_packet, CaptureEngine, CapturedPacket, FiveTuple, PacketMeta,
    TransportProtocol,
};
use bypath_core::config::{
    builtin_preset_rule, Config, GeneralConfig, ProxyConfig, ProxyKind,
};
use bypath_core::engine::ProxyEngine;
use bypath_core::process::{MockProcessTracker, ProcessInfo};
use bypath_core::upstream::socks5::{decode_udp_packet, encode_udp_packet};
use bypath_core::upstream::TargetAddr;
use dashmap::DashMap;
use smoltcp::iface::{Config as IfaceConfig, Interface, SocketSet};
use smoltcp::phy::{ChecksumCapabilities, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer as TcpSocketBuffer};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

type PacketChannelRx = mpsc::UnboundedReceiver<(Vec<u8>, PacketMeta)>;

/// In-memory mock capture engine capturing all outbound/inbound injected packets.
struct MockCaptureEngine {
    pid_map: DashMap<FiveTuple, u32>,
    tx_to_engine: Mutex<Option<mpsc::Sender<CapturedPacket>>>,
    inbound_replies_tx: mpsc::UnboundedSender<(Vec<u8>, PacketMeta)>,
    direct_outbound_tx: mpsc::UnboundedSender<(Vec<u8>, PacketMeta)>,
}

impl MockCaptureEngine {
    fn new() -> (Arc<Self>, PacketChannelRx, PacketChannelRx) {
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let (out_tx, out_rx) = mpsc::unbounded_channel();
        let engine = Arc::new(Self {
            pid_map: DashMap::new(),
            tx_to_engine: Mutex::new(None),
            inbound_replies_tx: in_tx,
            direct_outbound_tx: out_tx,
        });
        (engine, in_rx, out_rx)
    }

    fn map_tuple_pid(&self, tuple: FiveTuple, pid: u32) {
        self.pid_map.insert(tuple, pid);
    }

    async fn emit_outbound(&self, payload: Vec<u8>, iface_index: u32) {
        let tx = self.tx_to_engine.lock().unwrap().clone().expect("engine started");
        let meta = PacketMeta {
            iface_index,
            sub_iface_index: 0,
            outbound: true,
            is_ipv6: false,
        };
        tx.send(CapturedPacket { payload, meta }).await.unwrap();
    }
}

impl CaptureEngine for MockCaptureEngine {
    fn start(&self, tx: mpsc::Sender<CapturedPacket>) -> Result<()> {
        *self.tx_to_engine.lock().unwrap() = Some(tx);
        Ok(())
    }

    fn inject(&self, packet: &[u8], meta: &PacketMeta) -> Result<()> {
        if meta.outbound {
            let _ = self.direct_outbound_tx.send((packet.to_vec(), *meta));
        } else {
            let _ = self.inbound_replies_tx.send((packet.to_vec(), *meta));
        }
        Ok(())
    }

    fn lookup_pid(&self, tuple: &FiveTuple) -> Option<u32> {
        self.pid_map.get(tuple).map(|v| *v)
    }

    fn find_local_tcp_listener_pid(&self, _port: u16) -> Option<u32> {
        None
    }

    fn shutdown(&self) -> Result<()> {
        *self.tx_to_engine.lock().unwrap() = None;
        Ok(())
    }
}

/// Minimal `smoltcp` virtual NIC used to simulate a real client application's OS TCP/IP stack.
struct SimRx(Vec<u8>);
impl RxToken for SimRx {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(&mut self.0)
    }
}

struct SimTx<'a>(&'a mut VecDeque<Vec<u8>>);
impl<'a> TxToken for SimTx<'a> {
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

struct SimDevice {
    rx: VecDeque<Vec<u8>>,
    tx: VecDeque<Vec<u8>>,
}

impl Device for SimDevice {
    type RxToken<'a> = SimRx;
    type TxToken<'a> = SimTx<'a>;

    fn receive(&mut self, _: SmolInstant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let p = self.rx.pop_front()?;
        Some((SimRx(p), SimTx(&mut self.tx)))
    }

    fn transmit(&mut self, _: SmolInstant) -> Option<Self::TxToken<'_>> {
        Some(SimTx(&mut self.tx))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = 1500;
        caps.checksum = ChecksumCapabilities::default();
        caps
    }
}

/// Spawns a local mock SOCKS5 server (TCP CONNECT + UDP ASSOCIATE) that records the requested
/// target string (including ATYP=0x03 domain names) and echoes all tunneled bytes back prefixed
/// with `ECHO:`.
async fn spawn_mock_socks5_server() -> (
    SocketAddr,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let recorded_targets = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded_clone = Arc::clone(&recorded_targets);

    let handle = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let targets = Arc::clone(&recorded_clone);
            tokio::spawn(async move {
                let mut hdr = [0u8; 2];
                if stream.read_exact(&mut hdr).await.is_err() {
                    return;
                }
                let mut methods = vec![0u8; hdr[1] as usize];
                if stream.read_exact(&mut methods).await.is_err() {
                    return;
                }
                let _ = stream.write_all(&[0x05, 0x00]).await;

                let mut req = [0u8; 4];
                if stream.read_exact(&mut req).await.is_err() {
                    return;
                }
                let cmd = req[1];
                let atyp = req[3];
                let host = match atyp {
                    0x01 => {
                        let mut ip = [0u8; 4];
                        stream.read_exact(&mut ip).await.unwrap();
                        Ipv4Addr::from(ip).to_string()
                    }
                    0x03 => {
                        let mut len = [0u8; 1];
                        stream.read_exact(&mut len).await.unwrap();
                        let mut d = vec![0u8; len[0] as usize];
                        stream.read_exact(&mut d).await.unwrap();
                        String::from_utf8(d).unwrap()
                    }
                    _ => return,
                };
                let mut port_b = [0u8; 2];
                stream.read_exact(&mut port_b).await.unwrap();
                let port = u16::from_be_bytes(port_b);

                if cmd == 0x01 {
                    // TCP CONNECT
                    targets.lock().unwrap().push(format!("TCP:{host}:{port}"));
                    let _ = stream
                        .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
                        .await;
                    let mut buf = vec![0u8; 4096];
                    if let Ok(n) = stream.read(&mut buf).await {
                        if n > 0 {
                            let mut reply = b"ECHO:".to_vec();
                            reply.extend_from_slice(&buf[..n]);
                            let _ = stream.write_all(&reply).await;
                        }
                    }
                    let _ = stream.shutdown().await;
                } else if cmd == 0x03 {
                    // UDP ASSOCIATE
                    let udp_relay = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                    let relay_addr = udp_relay.local_addr().unwrap();
                    let mut reply = vec![0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1];
                    reply.extend_from_slice(&relay_addr.port().to_be_bytes());
                    let _ = stream.write_all(&reply).await;

                    let mut ubuf = vec![0u8; 4096];
                    if let Ok((n, client_udp)) = udp_relay.recv_from(&mut ubuf).await {
                        if let Ok((dst_addr, payload)) = decode_udp_packet(&ubuf[..n]) {
                            targets.lock().unwrap().push(format!("UDP:{dst_addr}"));
                            let mut resp_payload = b"UDP_ECHO:".to_vec();
                            resp_payload.extend_from_slice(payload);
                            let wrapped =
                                encode_udp_packet(&TargetAddr::Ip(dst_addr), &resp_payload)
                                    .unwrap();
                            let _ = udp_relay.send_to(&wrapped, client_udp).await;
                        }
                    }
                    // Keep control TCP stream briefly alive so reply arrives cleanly
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });
        }
    });

    (addr, recorded_targets, handle)
}

/// Spawns a local mock HTTP CONNECT proxy server recording requested authorities and echoing data.
async fn spawn_mock_http_proxy() -> (
    SocketAddr,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let recorded = Arc::new(Mutex::new(Vec::<String>::new()));
    let recorded_clone = Arc::clone(&recorded);

    let handle = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let rec = Arc::clone(&recorded_clone);
            tokio::spawn(async move {
                let mut hdr = Vec::new();
                let mut b = [0u8; 1];
                while !hdr.ends_with(b"\r\n\r\n") && hdr.len() < 4096 {
                    if stream.read_exact(&mut b).await.is_err() {
                        return;
                    }
                    hdr.push(b[0]);
                }
                let text = String::from_utf8_lossy(&hdr);
                let first_line = text.lines().next().unwrap_or_default();
                if let Some(authority) = first_line.split_whitespace().nth(1) {
                    rec.lock().unwrap().push(authority.to_string());
                }
                let _ = stream
                    .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                    .await;
                let mut buf = vec![0u8; 4096];
                if let Ok(n) = stream.read(&mut buf).await {
                    if n > 0 {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK").await;
                    }
                }
                let _ = stream.shutdown().await;
            });
        }
    });

    (addr, recorded, handle)
}

fn build_tls_client_hello(sni: &str) -> Vec<u8> {
    let hb = sni.as_bytes();
    let mut sni_ext = Vec::new();
    sni_ext.extend_from_slice(&((3 + hb.len()) as u16).to_be_bytes());
    sni_ext.push(0x00);
    sni_ext.extend_from_slice(&(hb.len() as u16).to_be_bytes());
    sni_ext.extend_from_slice(hb);

    let mut exts = Vec::new();
    exts.extend_from_slice(&0x0000u16.to_be_bytes());
    exts.extend_from_slice(&(sni_ext.len() as u16).to_be_bytes());
    exts.extend_from_slice(&sni_ext);

    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0x11; 32]);
    body.push(0);
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]);
    body.extend_from_slice(&[0x01, 0x00]);
    body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    body.extend_from_slice(&exts);

    let mut hs = vec![0x01, ((body.len() >> 16) & 0xFF) as u8, ((body.len() >> 8) & 0xFF) as u8, (body.len() & 0xFF) as u8];
    hs.extend_from_slice(&body);

    let mut rec = vec![0x16, 0x03, 0x01];
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

fn antigravity_process_tree() -> Arc<MockProcessTracker> {
    Arc::new(MockProcessTracker::with_processes([
        ProcessInfo {
            pid: 5000,
            parent_pid: 4,
            name: "Antigravity.exe".into(),
            exe_path: Some(r"C:\Users\dev\AppData\Local\Antigravity\Antigravity.exe".into()),
        },
        ProcessInfo {
            pid: 5001,
            parent_pid: 5000,
            name: "language_server_windows_x64.exe".into(),
            exe_path: Some(
                r"C:\Users\dev\AppData\Local\Antigravity\resources\language_server_windows_x64.exe"
                    .into(),
            ),
        },
        ProcessInfo {
            pid: 5002,
            parent_pid: 5001,
            name: "node.exe".into(),
            exe_path: Some(r"C:\Program Files\nodejs\node.exe".into()),
        },
        ProcessInfo {
            pid: 9000,
            parent_pid: 4,
            name: "unrelated.exe".into(),
            exe_path: Some(r"C:\Apps\unrelated.exe".into()),
        },
    ]))
}

#[tokio::test]
async fn e2e_antigravity_child_tls_sni_recovers_domain_over_poisoned_ip() {
    let (socks_addr, recorded_targets, socks_handle) = spawn_mock_socks5_server().await;

    let mut proxies = HashMap::new();
    proxies.insert(
        "local".to_string(),
        ProxyConfig {
            kind: ProxyKind::Socks5,
            address: socks_addr.to_string(),
            username: None,
            password: None,
            udp: true,
        },
    );

    let cfg = Config {
        general: GeneralConfig::default(),
        proxies,
        rules: vec![builtin_preset_rule("antigravity", "local").unwrap()],
    };

    let (mock_cap, mut inbound_rx, mut direct_rx) = MockCaptureEngine::new();
    let tracker = antigravity_process_tree();

    let engine = ProxyEngine::new(cfg, mock_cap.clone(), tracker).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let engine_task = tokio::spawn(engine.run(shutdown_rx));

    // Wait briefly for capture.start()
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Simulate Antigravity grandchild `node.exe` (PID 5002) connecting to a POISONED IP 198.51.100.42:443
    // with TLS SNI = `cloudcode-pa.googleapis.com`
    let client_ip = Ipv4Addr::new(10, 20, 30, 40);
    let poisoned_ip = Ipv4Addr::new(198, 51, 100, 42);
    let client_port = 54321u16;
    let dst_port = 443u16;

    mock_cap.map_tuple_pid(
        FiveTuple {
            protocol: TransportProtocol::Tcp,
            src_ip: IpAddr::V4(client_ip),
            src_port: client_port,
            dst_ip: IpAddr::V4(poisoned_ip),
            dst_port,
        },
        5002,
    );

    // Build client-side smoltcp stack
    let mut sim_dev = SimDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    };
    let origin = Instant::now();
    let mut sim_iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ip),
        &mut sim_dev,
        SmolInstant::from_millis(0),
    );
    sim_iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(IpAddress::v4(10, 20, 30, 40), 0));
    });
    let _ = sim_iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 20, 30, 1));

    let mut sim_sockets = SocketSet::new(vec![]);
    let sock = TcpSocket::new(
        TcpSocketBuffer::new(vec![0u8; 16384]),
        TcpSocketBuffer::new(vec![0u8; 16384]),
    );
    let sock_h = sim_sockets.add(sock);
    {
        let s = sim_sockets.get_mut::<TcpSocket>(sock_h);
        s.connect(
            sim_iface.context(),
            IpEndpoint::new(IpAddress::v4(198, 51, 100, 42), dst_port),
            client_port,
        )
        .unwrap();
    }

    let hello = build_tls_client_hello("cloudcode-pa.googleapis.com");
    let mut sent_hello = false;
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        let now = SmolInstant::from_millis(origin.elapsed().as_millis() as i64);
        sim_iface.poll(now, &mut sim_dev, &mut sim_sockets);

        while let Some(out_pkt) = sim_dev.tx.pop_front() {
            mock_cap.emit_outbound(out_pkt, 7).await;
        }

        while let Ok((in_pkt, meta)) = inbound_rx.try_recv() {
            assert!(!meta.outbound, "reply must be marked inbound");
            assert_eq!(meta.iface_index, 7, "reply must preserve original IfIdx");
            sim_dev.rx.push_back(in_pkt);
        }

        let s = sim_sockets.get_mut::<TcpSocket>(sock_h);
        if !sent_hello && s.may_send() {
            s.send_slice(&hello).unwrap();
            sent_hello = true;
        }
        if s.can_recv() {
            let mut tmp = vec![0u8; 4096];
            if let Ok(n) = s.recv_slice(&mut tmp) {
                received.extend_from_slice(&tmp[..n]);
            }
        }
        if received.len() >= 5 + hello.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert!(received.starts_with(b"ECHO:"));
    assert_eq!(&received[5..], hello.as_slice());

    // Prove the SOCKS5 proxy received the SNI DOMAIN NAME instead of the poisoned IP!
    {
        let targets = recorded_targets.lock().unwrap();
        assert!(
            targets.contains(&"TCP:cloudcode-pa.googleapis.com:443".to_string()),
            "expected recovered domain in SOCKS5 targets, got {targets:?}"
        );
    }

    // Also prove an unrelated process (PID 9000) bypasses the proxy directly!
    let unrelated_tuple = FiveTuple {
        protocol: TransportProtocol::Udp,
        src_ip: IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)),
        src_port: 60000,
        dst_ip: IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
        dst_port: 53,
    };
    mock_cap.map_tuple_pid(unrelated_tuple, 9000);
    let direct_pkt = build_udp_ip_packet(
        unrelated_tuple.src_ip,
        unrelated_tuple.src_port,
        unrelated_tuple.dst_ip,
        unrelated_tuple.dst_port,
        b"direct-bypass",
    )
    .unwrap();
    mock_cap.emit_outbound(direct_pkt.clone(), 9).await;

    let (bypassed, bypass_meta) = tokio::time::timeout(Duration::from_secs(2), direct_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(bypass_meta.outbound);
    assert_eq!(bypass_meta.iface_index, 9);
    assert_eq!(bypassed, direct_pkt);

    // Also verify SOCKS5 UDP ASSOCIATE end-to-end for Antigravity language_server (PID 5001)
    let udp_tuple = FiveTuple {
        protocol: TransportProtocol::Udp,
        src_ip: IpAddr::V4(Ipv4Addr::new(10, 20, 30, 40)),
        src_port: 51111,
        dst_ip: IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)),
        dst_port: 53,
    };
    mock_cap.map_tuple_pid(udp_tuple, 5001);
    let udp_out = build_udp_ip_packet(
        udp_tuple.src_ip,
        udp_tuple.src_port,
        udp_tuple.dst_ip,
        udp_tuple.dst_port,
        b"dns-over-socks5-udp",
    )
    .unwrap();
    mock_cap.emit_outbound(udp_out, 11).await;

    // Wait for the inbound UDP reply packet injected by UdpRelay
    let mut got_udp_reply = false;
    let udp_deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < udp_deadline {
        if let Ok(Some((pkt, meta))) =
            tokio::time::timeout(Duration::from_millis(100), inbound_rx.recv()).await
        {
            if let Some(parsed) = parse_ip_packet(&pkt) {
                if parsed.tuple.protocol == TransportProtocol::Udp
                    && parsed.tuple.dst_port == 51111
                {
                    assert!(!meta.outbound);
                    assert_eq!(meta.iface_index, 11);
                    assert_eq!(parsed.tuple.src_ip, IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
                    assert_eq!(parsed.tuple.src_port, 53);
                    assert_eq!(
                        &pkt[parsed.l4_payload_offset..],
                        b"UDP_ECHO:dns-over-socks5-udp"
                    );
                    got_udp_reply = true;
                    break;
                }
            }
        }
    }
    assert!(got_udp_reply, "expected UDP reply from SOCKS5 UDP ASSOCIATE");

    let _ = shutdown_tx.send(());
    let _ = engine_task.await;
    socks_handle.abort();
}

#[tokio::test]
async fn e2e_http_connect_proxy_recovers_http_host_and_drops_quic_when_no_udp() {
    let (http_addr, recorded, http_handle) = spawn_mock_http_proxy().await;

    let mut proxies = HashMap::new();
    proxies.insert(
        "http_up".to_string(),
        ProxyConfig {
            kind: ProxyKind::Http,
            address: http_addr.to_string(),
            username: None,
            password: None,
            udp: false,
        },
    );

    let cfg = Config {
        general: GeneralConfig {
            block_quic_when_no_udp: true,
            ..GeneralConfig::default()
        },
        proxies,
        rules: vec![builtin_preset_rule("antigravity", "http_up").unwrap()],
    };

    let (mock_cap, mut inbound_rx, mut direct_rx) = MockCaptureEngine::new();
    let tracker = antigravity_process_tree();
    let engine = ProxyEngine::new(cfg, mock_cap.clone(), tracker).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let engine_task = tokio::spawn(engine.run(shutdown_rx));
    tokio::time::sleep(Duration::from_millis(30)).await;

    // 1. Send HTTP/1.1 request to poisoned IP -> verify HTTP CONNECT receives `api.http.test:80`
    let client_ip = Ipv4Addr::new(10, 0, 1, 2);
    let dst_ip = Ipv4Addr::new(203, 0, 113, 9);
    let client_port = 52222;
    mock_cap.map_tuple_pid(
        FiveTuple {
            protocol: TransportProtocol::Tcp,
            src_ip: IpAddr::V4(client_ip),
            src_port: client_port,
            dst_ip: IpAddr::V4(dst_ip),
            dst_port: 80,
        },
        5000,
    );

    let mut sim_dev = SimDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    };
    let origin = Instant::now();
    let mut sim_iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ip),
        &mut sim_dev,
        SmolInstant::from_millis(0),
    );
    sim_iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(IpAddress::v4(10, 0, 1, 2), 0));
    });
    let _ = sim_iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 1, 1));
    let mut sim_sockets = SocketSet::new(vec![]);
    let sock_h = sim_sockets.add(TcpSocket::new(
        TcpSocketBuffer::new(vec![0u8; 8192]),
        TcpSocketBuffer::new(vec![0u8; 8192]),
    ));
    sim_sockets
        .get_mut::<TcpSocket>(sock_h)
        .connect(
            sim_iface.context(),
            IpEndpoint::new(IpAddress::v4(203, 0, 113, 9), 80),
            client_port,
        )
        .unwrap();

    let http_req = b"GET /health HTTP/1.1\r\nHost: api.http.test\r\n\r\n";
    let mut sent = false;
    let mut resp = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        let now = SmolInstant::from_millis(origin.elapsed().as_millis() as i64);
        sim_iface.poll(now, &mut sim_dev, &mut sim_sockets);
        while let Some(p) = sim_dev.tx.pop_front() {
            mock_cap.emit_outbound(p, 3).await;
        }
        while let Ok((in_p, _)) = inbound_rx.try_recv() {
            sim_dev.rx.push_back(in_p);
        }
        let s = sim_sockets.get_mut::<TcpSocket>(sock_h);
        if !sent && s.may_send() {
            s.send_slice(http_req).unwrap();
            sent = true;
        }
        if s.can_recv() {
            let mut tmp = vec![0u8; 1024];
            if let Ok(n) = s.recv_slice(&mut tmp) {
                resp.extend_from_slice(&tmp[..n]);
            }
        }
        if resp.ends_with(b"OK") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert!(String::from_utf8_lossy(&resp).contains("200 OK"));
    assert_eq!(
        recorded.lock().unwrap().as_slice(),
        &["api.http.test:80".to_string()]
    );

    // 2. Send UDP/443 QUIC packet from Antigravity -> must be dropped (not leaked directly!)
    let quic_tuple = FiveTuple {
        protocol: TransportProtocol::Udp,
        src_ip: IpAddr::V4(client_ip),
        src_port: 53333,
        dst_ip: IpAddr::V4(dst_ip),
        dst_port: 443,
    };
    mock_cap.map_tuple_pid(quic_tuple, 5000);
    let mut quic_initial = vec![0u8; 32];
    quic_initial[0] = 0xC3;
    quic_initial[1..5].copy_from_slice(&0x0000_0001u32.to_be_bytes());
    quic_initial[5] = 8;
    let quic_pkt = build_udp_ip_packet(
        quic_tuple.src_ip,
        quic_tuple.src_port,
        quic_tuple.dst_ip,
        quic_tuple.dst_port,
        &quic_initial,
    )
    .unwrap();
    mock_cap.emit_outbound(quic_pkt, 3).await;

    // Assert it did NOT leak directly outbound
    assert!(
        tokio::time::timeout(Duration::from_millis(150), direct_rx.recv())
            .await
            .is_err(),
        "QUIC packet must be dropped instead of leaking direct when upstream UDP is disabled"
    );

    let _ = shutdown_tx.send(());
    let _ = engine_task.await;
    http_handle.abort();
}

#[tokio::test]
async fn e2e_concurrent_same_dst_with_windows_checksum_and_lso_offload() {
    let (socks_addr, recorded_targets, socks_handle) = spawn_mock_socks5_server().await;

    let mut proxies = HashMap::new();
    proxies.insert(
        "local".to_string(),
        ProxyConfig {
            kind: ProxyKind::Socks5,
            address: socks_addr.to_string(),
            username: None,
            password: None,
            udp: true,
        },
    );

    let cfg = Config {
        general: GeneralConfig::default(),
        proxies,
        rules: vec![builtin_preset_rule("antigravity", "local").unwrap()],
    };

    let (mock_cap, mut inbound_rx, _direct_rx) = MockCaptureEngine::new();
    let tracker = antigravity_process_tree();
    let engine = ProxyEngine::new(cfg, mock_cap.clone(), tracker).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let engine_task = tokio::spawn(engine.run(shutdown_rx));
    tokio::time::sleep(Duration::from_millis(30)).await;

    let client_ip = Ipv4Addr::new(10, 88, 0, 2);
    let shared_dst = Ipv4Addr::new(198, 51, 100, 99);
    let dst_port = 443u16;

    let mut sim_dev = SimDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    };
    let origin = Instant::now();
    let mut sim_iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ip),
        &mut sim_dev,
        SmolInstant::from_millis(0),
    );
    sim_iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(IpAddress::v4(10, 88, 0, 2), 0));
    });
    let _ = sim_iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 88, 0, 1));

    let mut sim_sockets = SocketSet::new(vec![]);
    let mut handles = Vec::new();
    let mut hellos = Vec::new();
    let mut sent = [false; 5];
    let mut recvd = vec![Vec::<u8>::new(); 5];

    for i in 0..5u16 {
        let sport = 56000 + i;
        mock_cap.map_tuple_pid(
            FiveTuple {
                protocol: TransportProtocol::Tcp,
                src_ip: IpAddr::V4(client_ip),
                src_port: sport,
                dst_ip: IpAddr::V4(shared_dst),
                dst_port,
            },
            5001,
        );
        let h = sim_sockets.add(TcpSocket::new(
            TcpSocketBuffer::new(vec![0u8; 8192]),
            TcpSocketBuffer::new(vec![0u8; 8192]),
        ));
        sim_sockets
            .get_mut::<TcpSocket>(h)
            .connect(
                sim_iface.context(),
                IpEndpoint::new(IpAddress::v4(198, 51, 100, 99), dst_port),
                sport,
            )
            .unwrap();
        handles.push(h);
        hellos.push(build_tls_client_hello(&format!("s{i}.googleapis.com")));
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let now = SmolInstant::from_millis(origin.elapsed().as_millis() as i64);
        sim_iface.poll(now, &mut sim_dev, &mut sim_sockets);

        while let Some(mut out_pkt) = sim_dev.tx.pop_front() {
            // Simulate Windows NIC Checksum Offload + LSOv2 (zeroed IP TotalLength, IP checksum, TCP checksum)
            if out_pkt.len() >= 40 && (out_pkt[0] >> 4) == 4 {
                out_pkt[2] = 0;
                out_pkt[3] = 0;
                out_pkt[10] = 0;
                out_pkt[11] = 0;
                let ihl = ((out_pkt[0] & 0x0F) as usize) * 4;
                if out_pkt.len() >= ihl + 18 {
                    out_pkt[ihl + 16] = 0;
                    out_pkt[ihl + 17] = 0;
                }
            }
            mock_cap.emit_outbound(out_pkt, 15).await;
        }

        while let Ok((in_pkt, meta)) = inbound_rx.try_recv() {
            assert!(!meta.outbound);
            assert_eq!(meta.iface_index, 15);
            sim_dev.rx.push_back(in_pkt);
        }

        let mut all_done = true;
        for i in 0..5 {
            let s = sim_sockets.get_mut::<TcpSocket>(handles[i]);
            if !sent[i] && s.may_send() {
                s.send_slice(&hellos[i]).unwrap();
                sent[i] = true;
            }
            if s.can_recv() {
                let mut tmp = vec![0u8; 4096];
                if let Ok(n) = s.recv_slice(&mut tmp) {
                    recvd[i].extend_from_slice(&tmp[..n]);
                }
            }
            if recvd[i].len() < 5 + hellos[i].len() {
                all_done = false;
            }
        }
        if all_done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    for i in 0..5 {
        assert!(
            recvd[i].starts_with(b"ECHO:"),
            "stream {i} did not receive full echo"
        );
        assert_eq!(&recvd[i][5..], hellos[i].as_slice());
    }

    {
        let targets = recorded_targets.lock().unwrap();
        for i in 0..5 {
            let expected = format!("TCP:s{i}.googleapis.com:443");
            assert!(
                targets.contains(&expected),
                "missing {expected} in {targets:?}"
            );
        }
    }

    let _ = shutdown_tx.send(());
    let _ = engine_task.await;
    socks_handle.abort();
}

#[tokio::test]
async fn e2e_telemetry_and_hot_reload_rules_live() {
    use bypath_core::config::{RuleAction, RuleConfig};
    use bypath_core::telemetry::ConnectionState;

    let (socks_addr, _recorded_targets, socks_handle) = spawn_mock_socks5_server().await;

    let mut proxies = HashMap::new();
    proxies.insert(
        "local".to_string(),
        ProxyConfig {
            kind: ProxyKind::Socks5,
            address: socks_addr.to_string(),
            username: None,
            password: None,
            udp: true,
        },
    );

    // Start with NO rule matching unrelated.exe (PID 9000)
    let initial_cfg = Config {
        general: GeneralConfig::default(),
        proxies: proxies.clone(),
        rules: vec![builtin_preset_rule("antigravity", "local").unwrap()],
    };

    let (mock_cap, mut inbound_rx, mut direct_rx) = MockCaptureEngine::new();
    let tracker = antigravity_process_tree();
    let engine = ProxyEngine::new(initial_cfg, mock_cap.clone(), tracker).unwrap();
    let handle = engine.handle();
    let telemetry = handle.telemetry();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let engine_task = tokio::spawn(engine.run(shutdown_rx));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(telemetry.stats_snapshot().running);

    // Before hot-reload, PID 9000 UDP goes direct
    let udp_tuple = FiveTuple {
        protocol: TransportProtocol::Udp,
        src_ip: IpAddr::V4(Ipv4Addr::new(10, 50, 0, 2)),
        src_port: 54001,
        dst_ip: IpAddr::V4(Ipv4Addr::new(8, 8, 4, 4)),
        dst_port: 53,
    };
    mock_cap.map_tuple_pid(udp_tuple, 9000);
    let pkt1 = build_udp_ip_packet(
        udp_tuple.src_ip,
        udp_tuple.src_port,
        udp_tuple.dst_ip,
        udp_tuple.dst_port,
        b"before-reload",
    )
    .unwrap();
    mock_cap.emit_outbound(pkt1.clone(), 4).await;
    let (bypassed, _) = tokio::time::timeout(Duration::from_secs(2), direct_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(bypassed, pkt1);

    // Hot-reload config to add a rule proxying unrelated.exe (PID 9000)
    let updated_cfg = Config {
        general: GeneralConfig::default(),
        proxies,
        rules: vec![
            builtin_preset_rule("antigravity", "local").unwrap(),
            RuleConfig {
                name: "Hot Reload Unrelated".into(),
                enabled: true,
                process_names: vec!["unrelated.exe".into()],
                path_prefixes: vec![],
                path_contains: vec![],
                include_children: false,
                dst_cidrs: vec![],
                dst_ports: vec![],
                protocols: vec![],
                domains: vec![],
                action: RuleAction::Proxy,
                proxy: Some("local".into()),
            },
        ],
    };
    handle.reload_config(updated_cfg).unwrap();

    // Send another UDP packet on the SAME 5-tuple -> must now be proxied via SOCKS5 UDP ASSOCIATE!
    let pkt2 = build_udp_ip_packet(
        udp_tuple.src_ip,
        udp_tuple.src_port,
        udp_tuple.dst_ip,
        udp_tuple.dst_port,
        b"after-reload",
    )
    .unwrap();
    mock_cap.emit_outbound(pkt2, 4).await;

    let mut got_proxied_udp = false;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some((pkt, _))) =
            tokio::time::timeout(Duration::from_millis(100), inbound_rx.recv()).await
        {
            if let Some(parsed) = parse_ip_packet(&pkt) {
                if parsed.tuple.dst_port == 54001
                    && &pkt[parsed.l4_payload_offset..] == b"UDP_ECHO:after-reload"
                {
                    got_proxied_udp = true;
                    break;
                }
            }
        }
    }
    assert!(
        got_proxied_udp,
        "hot-reloaded rule must immediately proxy UDP packets for unrelated.exe"
    );

    // Telemetry must have captured the proxied connection & bytes!
    let conns = telemetry.list_connections();
    assert!(
        conns
            .iter()
            .any(|c| c.process_name == "unrelated.exe" && c.bytes_tx > 0 && c.bytes_rx > 0),
        "expected unrelated.exe connection with tx/rx bytes in telemetry: {conns:?}"
    );
    let stats = telemetry.stats_snapshot();
    assert!(stats.total_bytes_tx >= b"after-reload".len() as u64);
    assert!(stats.total_bytes_rx >= b"UDP_ECHO:after-reload".len() as u64);

    let _ = shutdown_tx.send(());
    let _ = engine_task.await;
    assert!(!telemetry.stats_snapshot().running);
    assert!(telemetry
        .list_connections()
        .iter()
        .all(|c| c.state != ConnectionState::Active));
    socks_handle.abort();
}

#[tokio::test]
async fn e2e_domain_block_aborts_matching_sni_and_allows_other_domains() {
    use bypath_core::config::{RuleAction, RuleConfig};
    use bypath_core::telemetry::ConnectionState;

    let (socks_addr, recorded_targets, socks_handle) = spawn_mock_socks5_server().await;

    let mut proxies = HashMap::new();
    proxies.insert(
        "local".to_string(),
        ProxyConfig {
            kind: ProxyKind::Socks5,
            address: socks_addr.to_string(),
            username: None,
            password: None,
            udp: true,
        },
    );

    let cfg = Config {
        general: GeneralConfig::default(),
        proxies,
        rules: vec![
            builtin_preset_rule("antigravity", "local").unwrap(),
            RuleConfig {
                name: "Block Trackers".into(),
                enabled: true,
                process_names: vec![],
                path_prefixes: vec![],
                path_contains: vec![],
                include_children: true,
                dst_cidrs: vec![],
                dst_ports: vec![],
                protocols: vec![],
                domains: vec!["*.blocked.test".into(), "telemetry.example.org".into()],
                action: RuleAction::Block,
                proxy: None,
            },
        ],
    };

    let (mock_cap, mut inbound_rx, _direct_rx) = MockCaptureEngine::new();
    let tracker = antigravity_process_tree();
    let engine = ProxyEngine::new(cfg, mock_cap.clone(), tracker).unwrap();
    let telemetry = engine.handle().telemetry();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let engine_task = tokio::spawn(engine.run(shutdown_rx));
    tokio::time::sleep(Duration::from_millis(30)).await;

    let client_ip = Ipv4Addr::new(10, 77, 0, 2);
    let dst_ip = Ipv4Addr::new(198, 51, 100, 77);
    let dst_port = 443u16;

    let mut sim_dev = SimDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    };
    let origin = Instant::now();
    let mut sim_iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ip),
        &mut sim_dev,
        SmolInstant::from_millis(0),
    );
    sim_iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(IpAddress::v4(10, 77, 0, 2), 0));
    });
    let _ = sim_iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 77, 0, 1));

    let mut sim_sockets = SocketSet::new(vec![]);
    let port_blocked = 57001u16;
    let port_allowed = 57002u16;

    for port in [port_blocked, port_allowed] {
        mock_cap.map_tuple_pid(
            FiveTuple {
                protocol: TransportProtocol::Tcp,
                src_ip: IpAddr::V4(client_ip),
                src_port: port,
                dst_ip: IpAddr::V4(dst_ip),
                dst_port,
            },
            5001,
        );
    }

    let h_blocked = sim_sockets.add(TcpSocket::new(
        TcpSocketBuffer::new(vec![0u8; 8192]),
        TcpSocketBuffer::new(vec![0u8; 8192]),
    ));
    sim_sockets
        .get_mut::<TcpSocket>(h_blocked)
        .connect(
            sim_iface.context(),
            IpEndpoint::new(IpAddress::v4(198, 51, 100, 77), dst_port),
            port_blocked,
        )
        .unwrap();

    let h_allowed = sim_sockets.add(TcpSocket::new(
        TcpSocketBuffer::new(vec![0u8; 8192]),
        TcpSocketBuffer::new(vec![0u8; 8192]),
    ));
    sim_sockets
        .get_mut::<TcpSocket>(h_allowed)
        .connect(
            sim_iface.context(),
            IpEndpoint::new(IpAddress::v4(198, 51, 100, 77), dst_port),
            port_allowed,
        )
        .unwrap();

    let hello_blocked = build_tls_client_hello("ads.blocked.test");
    let hello_allowed = build_tls_client_hello("allowed.googleapis.com");
    let mut sent_blocked = false;
    let mut sent_allowed = false;
    let mut recvd_allowed = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        let now = SmolInstant::from_millis(origin.elapsed().as_millis() as i64);
        sim_iface.poll(now, &mut sim_dev, &mut sim_sockets);
        while let Some(p) = sim_dev.tx.pop_front() {
            mock_cap.emit_outbound(p, 8).await;
        }
        while let Ok((in_p, _)) = inbound_rx.try_recv() {
            sim_dev.rx.push_back(in_p);
        }

        let sb = sim_sockets.get_mut::<TcpSocket>(h_blocked);
        if !sent_blocked && sb.may_send() {
            sb.send_slice(&hello_blocked).unwrap();
            sent_blocked = true;
        }
        let sa = sim_sockets.get_mut::<TcpSocket>(h_allowed);
        if !sent_allowed && sa.may_send() {
            sa.send_slice(&hello_allowed).unwrap();
            sent_allowed = true;
        }
        if sa.can_recv() {
            let mut tmp = vec![0u8; 4096];
            if let Ok(n) = sa.recv_slice(&mut tmp) {
                recvd_allowed.extend_from_slice(&tmp[..n]);
            }
        }

        let blocked_recorded = telemetry.list_connections().iter().any(|c| {
            c.sniffed_host.as_deref() == Some("ads.blocked.test")
                && c.state == ConnectionState::Blocked
        });
        if blocked_recorded && recvd_allowed.len() >= 5 + hello_allowed.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert!(recvd_allowed.starts_with(b"ECHO:"));
    assert_eq!(&recvd_allowed[5..], hello_allowed.as_slice());

    let targets = recorded_targets.lock().unwrap().clone();
    assert!(
        targets.contains(&"TCP:allowed.googleapis.com:443".to_string()),
        "allowed domain must reach SOCKS5 proxy: {targets:?}"
    );
    assert!(
        !targets.iter().any(|t| t.contains("blocked.test")),
        "blocked domain must NEVER reach SOCKS5 upstream: {targets:?}"
    );

    let conns = telemetry.list_connections();
    let blocked_rec = conns
        .iter()
        .find(|c| c.sniffed_host.as_deref() == Some("ads.blocked.test"))
        .expect("blocked domain connection must be present in telemetry");
    assert_eq!(blocked_rec.state, ConnectionState::Blocked);
    assert_eq!(blocked_rec.rule_name, "Block Trackers");
    assert!(blocked_rec.proxy_name.is_none());
    assert!(telemetry.stats_snapshot().blocked_connections >= 1);

    let _ = shutdown_tx.send(());
    let _ = engine_task.await;
    socks_handle.abort();
}

#[test]
fn config_example_toml_parses_and_validates() {
    let example_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config.example.toml");
    let cfg = Config::from_file(&example_path).expect("config.example.toml must parse cleanly");
    assert!(cfg.proxies.contains_key("local_socks"));
    assert!(cfg.proxies.contains_key("local_http"));
    assert!(!cfg.rules.is_empty());
}

fn build_large_post_quantum_client_hello(sni: &str, padding_bytes: usize) -> Vec<u8> {
    let hb = sni.as_bytes();
    let mut exts = Vec::new();

    // Place a large ML-KEM / padding extension BEFORE server_name so SNI falls into TCP segment 2.
    exts.extend_from_slice(&0x0015u16.to_be_bytes()); // TLS padding extension
    exts.extend_from_slice(&(padding_bytes as u16).to_be_bytes());
    exts.extend(std::iter::repeat_n(0x00u8, padding_bytes));

    let mut sni_ext = Vec::new();
    sni_ext.extend_from_slice(&((3 + hb.len()) as u16).to_be_bytes());
    sni_ext.push(0x00);
    sni_ext.extend_from_slice(&(hb.len() as u16).to_be_bytes());
    sni_ext.extend_from_slice(hb);

    exts.extend_from_slice(&0x0000u16.to_be_bytes());
    exts.extend_from_slice(&(sni_ext.len() as u16).to_be_bytes());
    exts.extend_from_slice(&sni_ext);

    let mut body = Vec::new();
    body.extend_from_slice(&[0x03, 0x03]);
    body.extend_from_slice(&[0x22; 32]);
    body.push(0);
    body.extend_from_slice(&[0x00, 0x02, 0x13, 0x01]);
    body.extend_from_slice(&[0x01, 0x00]);
    body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
    body.extend_from_slice(&exts);

    let mut hs = vec![
        0x01,
        ((body.len() >> 16) & 0xFF) as u8,
        ((body.len() >> 8) & 0xFF) as u8,
        (body.len() & 0xFF) as u8,
    ];
    hs.extend_from_slice(&body);

    let mut rec = vec![0x16, 0x03, 0x01];
    rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
    rec.extend_from_slice(&hs);
    rec
}

#[tokio::test]
async fn e2e_fragmented_post_quantum_tls_client_hello_recovers_sni() {
    let (socks_addr, recorded_targets, socks_handle) = spawn_mock_socks5_server().await;

    let mut proxies = HashMap::new();
    proxies.insert(
        "local".to_string(),
        ProxyConfig {
            kind: ProxyKind::Socks5,
            address: socks_addr.to_string(),
            username: None,
            password: None,
            udp: true,
        },
    );

    let cfg = Config {
        general: GeneralConfig::default(),
        proxies,
        rules: vec![builtin_preset_rule("antigravity", "local").unwrap()],
    };

    let (mock_cap, mut inbound_rx, _direct_rx) = MockCaptureEngine::new();
    let tracker = antigravity_process_tree();
    let engine = ProxyEngine::new(cfg, mock_cap.clone(), tracker).unwrap();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
    let engine_task = tokio::spawn(engine.run(shutdown_rx));
    tokio::time::sleep(Duration::from_millis(30)).await;

    let client_ip = Ipv4Addr::new(10, 77, 0, 2);
    let poisoned_ip = Ipv4Addr::new(198, 51, 100, 77);
    let client_port = 57777u16;
    mock_cap.map_tuple_pid(
        FiveTuple {
            protocol: TransportProtocol::Tcp,
            src_ip: IpAddr::V4(client_ip),
            src_port: client_port,
            dst_ip: IpAddr::V4(poisoned_ip),
            dst_port: 443,
        },
        5001,
    );

    let mut sim_dev = SimDevice {
        rx: VecDeque::new(),
        tx: VecDeque::new(),
    };
    let origin = Instant::now();
    let mut sim_iface = Interface::new(
        IfaceConfig::new(HardwareAddress::Ip),
        &mut sim_dev,
        SmolInstant::from_millis(0),
    );
    sim_iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(IpAddress::v4(10, 77, 0, 2), 0));
    });
    let _ = sim_iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 77, 0, 1));
    let mut sim_sockets = SocketSet::new(vec![]);
    let sock_h = sim_sockets.add(TcpSocket::new(
        TcpSocketBuffer::new(vec![0u8; 16384]),
        TcpSocketBuffer::new(vec![0u8; 16384]),
    ));
    sim_sockets
        .get_mut::<TcpSocket>(sock_h)
        .connect(
            sim_iface.context(),
            IpEndpoint::new(IpAddress::v4(198, 51, 100, 77), 443),
            client_port,
        )
        .unwrap();

    // 1800-byte ClientHello guarantees >= 2 TCP segments over 1500-byte MTU, placing SNI in segment 2.
    let large_hello = build_large_post_quantum_client_hello("pq-sni.googleapis.com", 1600);
    assert!(large_hello.len() > 1650);

    let mut offset = 0usize;
    let mut received = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);

    while Instant::now() < deadline {
        let now = SmolInstant::from_millis(origin.elapsed().as_millis() as i64);
        sim_iface.poll(now, &mut sim_dev, &mut sim_sockets);
        while let Some(p) = sim_dev.tx.pop_front() {
            mock_cap.emit_outbound(p, 5).await;
        }
        while let Ok((in_p, _)) = inbound_rx.try_recv() {
            sim_dev.rx.push_back(in_p);
        }

        let s = sim_sockets.get_mut::<TcpSocket>(sock_h);
        if offset < large_hello.len() && s.may_send() {
            // Deliberately split sends into two chunks separated across poll ticks.
            let chunk_end = if offset == 0 {
                700.min(large_hello.len())
            } else {
                large_hello.len()
            };
            if let Ok(n) = s.send_slice(&large_hello[offset..chunk_end]) {
                offset += n;
            }
        }
        if s.can_recv() {
            let mut tmp = vec![0u8; 4096];
            if let Ok(n) = s.recv_slice(&mut tmp) {
                received.extend_from_slice(&tmp[..n]);
            }
        }
        if received.len() >= 5 + large_hello.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    assert!(received.starts_with(b"ECHO:"));
    assert_eq!(&received[5..], large_hello.as_slice());

    let targets = recorded_targets.lock().unwrap().clone();
    assert!(
        targets.contains(&"TCP:pq-sni.googleapis.com:443".to_string()),
        "expected fragmented post-quantum SNI in SOCKS5 target list, got {targets:?}"
    );

    let _ = shutdown_tx.send(());
    let _ = engine_task.await;
    socks_handle.abort();
}
