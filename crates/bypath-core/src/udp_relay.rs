use crate::capture::{build_udp_ip_packet, CaptureEngine, FiveTuple, PacketMeta};
use crate::config::ProxyConfig;
use crate::engine::RuntimeConfig;
use crate::process::ProcessTracker;
use crate::sniff::{sniff_udp_payload, SniffResult};
use crate::telemetry::{
    ConnectionAction, ConnectionProtocol, NewConnectionParams, TelemetryHub,
};
use crate::upstream::socks5::{associate_udp, decode_udp_packet, encode_udp_packet};
use crate::upstream::TargetAddr;
use dashmap::DashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

const UDP_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct UdpSessionKey {
    src_ip: IpAddr,
    src_port: u16,
    proxy_name: String,
}

struct UdpOutboundDatagram {
    dst_ip: IpAddr,
    dst_port: u16,
    payload: Vec<u8>,
    meta: PacketMeta,
}

/// Metadata passed with an intercepted outbound UDP packet.
pub struct UdpPacketContext {
    pub pid: Option<u32>,
    pub process_name: String,
    pub exe_path: Option<String>,
    pub process_label: String,
    pub rule_name: String,
    pub proxy_name: String,
    pub proxy: ProxyConfig,
}

/// Manages per-source-socket SOCKS5 UDP ASSOCIATE sessions and QUIC fallback blocking.
pub struct UdpRelay {
    capture: Arc<dyn CaptureEngine>,
    tracker: Arc<dyn ProcessTracker>,
    runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
    block_quic_when_no_udp: Arc<AtomicBool>,
    telemetry: Arc<TelemetryHub>,
    sessions: Arc<DashMap<UdpSessionKey, mpsc::Sender<UdpOutboundDatagram>>>,
    quic_logged: DashMap<FiveTuple, Instant>,
}

impl UdpRelay {
    pub(crate) fn new(
        capture: Arc<dyn CaptureEngine>,
        tracker: Arc<dyn ProcessTracker>,
        runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
        block_quic_when_no_udp: Arc<AtomicBool>,
        telemetry: Arc<TelemetryHub>,
    ) -> Self {
        Self {
            capture,
            tracker,
            runtime,
            block_quic_when_no_udp,
            telemetry,
            sessions: Arc::new(DashMap::new()),
            quic_logged: DashMap::new(),
        }
    }

    pub async fn handle_outbound_udp(
        &self,
        tuple: FiveTuple,
        payload: Vec<u8>,
        meta: PacketMeta,
        ctx: UdpPacketContext,
    ) {
        let is_quic = tuple.dst_port == 443
            || matches!(sniff_udp_payload(&payload), Some(SniffResult::QuicInitial));

        // When domain-blocking rules are active for this flow, drop encrypted QUIC/443
        // so clients immediately fall back to inspectable TLS/TCP where SNI can be enforced.
        if is_quic {
            let needs_domain_inspection = {
                let rt = self.runtime.read().expect("runtime config lock poisoned");
                rt.rules
                    .has_domain_block_rules_for_flow(ctx.pid, &tuple, self.tracker.as_ref())
            };
            if needs_domain_inspection {
                if self.should_log_quic(&tuple) {
                    info!(
                        proc = %ctx.process_label,
                        proto = "UDP/QUIC",
                        dst = %SocketAddr::new(tuple.dst_ip, tuple.dst_port),
                        "dropping QUIC packet to enforce TCP SNI domain inspection"
                    );
                    self.telemetry.record_blocked(
                        NewConnectionParams {
                            pid: ctx.pid,
                            process_name: ctx.process_name,
                            exe_path: ctx.exe_path,
                            protocol: ConnectionProtocol::Quic,
                            src_addr: SocketAddr::new(tuple.src_ip, tuple.src_port).to_string(),
                            dst_ip: tuple.dst_ip.to_string(),
                            dst_port: tuple.dst_port,
                            rule_name: ctx.rule_name,
                            proxy_name: Some(ctx.proxy_name),
                            action: ConnectionAction::Block,
                        },
                        Some("QUIC dropped for domain rule inspection (TCP fallback)".into()),
                    );
                }
                return;
            }
        }

        if !ctx.proxy.udp {
            if is_quic && self.block_quic_when_no_udp.load(Ordering::Relaxed) {
                if self.should_log_quic(&tuple) {
                    info!(
                        proc = %ctx.process_label,
                        proto = "UDP/QUIC",
                        dst = %SocketAddr::new(tuple.dst_ip, tuple.dst_port),
                        proxy = %ctx.proxy_name,
                        "dropping QUIC packet (upstream UDP disabled) to trigger fast TCP fallback"
                    );
                    self.telemetry.record_blocked(
                        NewConnectionParams {
                            pid: ctx.pid,
                            process_name: ctx.process_name,
                            exe_path: ctx.exe_path,
                            protocol: ConnectionProtocol::Quic,
                            src_addr: SocketAddr::new(tuple.src_ip, tuple.src_port).to_string(),
                            dst_ip: tuple.dst_ip.to_string(),
                            dst_port: tuple.dst_port,
                            rule_name: ctx.rule_name,
                            proxy_name: Some(ctx.proxy_name),
                            action: ConnectionAction::Block,
                        },
                        Some("QUIC blocked for TCP fallback (upstream UDP disabled)".into()),
                    );
                }
            } else {
                debug!(
                    proc = %ctx.process_label,
                    dst = %SocketAddr::new(tuple.dst_ip, tuple.dst_port),
                    proxy = %ctx.proxy_name,
                    "dropping UDP packet because upstream proxy does not enable UDP"
                );
            }
            return;
        }

        let key = UdpSessionKey {
            src_ip: tuple.src_ip,
            src_port: tuple.src_port,
            proxy_name: ctx.proxy_name.clone(),
        };

        let datagram = UdpOutboundDatagram {
            dst_ip: tuple.dst_ip,
            dst_port: tuple.dst_port,
            payload,
            meta,
        };

        if let Some(existing) = self.sessions.get(&key) {
            if existing.try_send(datagram).is_ok() {
                return;
            }
            drop(existing);
            self.sessions.remove(&key);
            return;
        }

        let conn_id = self.telemetry.open_connection(NewConnectionParams {
            pid: ctx.pid,
            process_name: ctx.process_name,
            exe_path: ctx.exe_path,
            protocol: if is_quic {
                ConnectionProtocol::Quic
            } else {
                ConnectionProtocol::Udp
            },
            src_addr: SocketAddr::new(tuple.src_ip, tuple.src_port).to_string(),
            dst_ip: tuple.dst_ip.to_string(),
            dst_port: tuple.dst_port,
            rule_name: ctx.rule_name,
            proxy_name: Some(ctx.proxy_name),
            action: ConnectionAction::Proxy,
        });

        let (tx, rx) = mpsc::channel::<UdpOutboundDatagram>(128);
        let _ = tx.try_send(datagram);
        self.sessions.insert(key.clone(), tx);

        let sessions = Arc::clone(&self.sessions);
        let capture = Arc::clone(&self.capture);
        let telemetry = Arc::clone(&self.telemetry);

        tokio::spawn(async move {
            run_udp_session(
                conn_id,
                key.clone(),
                ctx.proxy,
                ctx.process_label,
                rx,
                capture,
                telemetry,
            )
            .await;
            sessions.remove(&key);
        });
    }

    fn should_log_quic(&self, tuple: &FiveTuple) -> bool {
        let now = Instant::now();
        if let Some(prev) = self.quic_logged.get(tuple) {
            if now.duration_since(*prev) < Duration::from_secs(30) {
                return false;
            }
        }
        self.quic_logged.insert(*tuple, now);
        true
    }
}

async fn run_udp_session(
    conn_id: u64,
    key: UdpSessionKey,
    proxy: ProxyConfig,
    process_label: String,
    mut rx: mpsc::Receiver<UdpOutboundDatagram>,
    capture: Arc<dyn CaptureEngine>,
    telemetry: Arc<TelemetryHub>,
) {
    let assoc = match associate_udp(&proxy).await {
        Ok(a) => a,
        Err(err) => {
            let err_msg = err.to_string();
            warn!(
                proc = %process_label,
                proxy = %key.proxy_name,
                error = %err_msg,
                "SOCKS5 UDP ASSOCIATE failed"
            );
            telemetry.close_connection(conn_id, Some(err_msg));
            return;
        }
    };

    info!(
        proc = %process_label,
        proto = "UDP",
        src = %SocketAddr::new(key.src_ip, key.src_port),
        relay = %assoc.relay_addr,
        proxy = %key.proxy_name,
        "established SOCKS5 UDP association"
    );

    let mut last_meta: Option<PacketMeta> = None;
    let mut recv_buf = vec![0u8; 65_535];
    let mut close_err: Option<String> = None;

    loop {
        tokio::select! {
            maybe_out = rx.recv() => {
                match maybe_out {
                    Some(dgram) => {
                        last_meta = Some(dgram.meta);
                        let payload_len = dgram.payload.len() as u64;
                        let target = TargetAddr::Ip(SocketAddr::new(dgram.dst_ip, dgram.dst_port));
                        match encode_udp_packet(&target, &dgram.payload) {
                            Ok(encoded) => {
                                if let Err(err) = assoc.udp_socket.send_to(&encoded, assoc.relay_addr).await {
                                    debug!(?err, "failed sending SOCKS5 UDP datagram");
                                    close_err = Some(err.to_string());
                                    break;
                                }
                                telemetry.add_bytes(conn_id, payload_len, 0);
                            }
                            Err(err) => debug!(?err, "failed encoding SOCKS5 UDP datagram"),
                        }
                    }
                    None => break,
                }
            }
            recv_res = assoc.udp_socket.recv_from(&mut recv_buf) => {
                match recv_res {
                    Ok((n, _from)) => {
                        if let Some(meta) = last_meta {
                            match decode_udp_packet(&recv_buf[..n]) {
                                Ok((remote_addr, payload)) => {
                                    telemetry.add_bytes(conn_id, 0, payload.len() as u64);
                                    if let Some(raw_ip) = build_udp_ip_packet(
                                        remote_addr.ip(),
                                        remote_addr.port(),
                                        key.src_ip,
                                        key.src_port,
                                        payload,
                                    ) {
                                        let reply_meta = meta.as_inbound_reply();
                                        if let Err(err) = capture.inject(&raw_ip, &reply_meta) {
                                            debug!(?err, "failed injecting UDP reply packet");
                                        }
                                    }
                                }
                                Err(err) => debug!(?err, "invalid SOCKS5 UDP reply packet"),
                            }
                        }
                    }
                    Err(err) => {
                        debug!(?err, "SOCKS5 UDP socket read error");
                        close_err = Some(err.to_string());
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(UDP_IDLE_TIMEOUT) => {
                debug!(
                    src = %SocketAddr::new(key.src_ip, key.src_port),
                    "expiring idle SOCKS5 UDP session"
                );
                break;
            }
        }
    }

    telemetry.close_connection(conn_id, close_err);
}
