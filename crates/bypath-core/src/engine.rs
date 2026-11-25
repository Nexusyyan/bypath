use crate::capture::{parse_ip_packet, CaptureEngine, CapturedPacket, FiveTuple, TransportProtocol};
use crate::config::{Config, ProxyConfig};
use crate::netstack::{NetstackInput, TcpNetstack};
use crate::process::ProcessTracker;
use crate::rules::{RouteDecision, RuleEngine};
use crate::telemetry::{
    ConnectionAction, ConnectionProtocol, NewConnectionParams, TelemetryHub,
};
use crate::udp_relay::{UdpPacketContext, UdpRelay};
use anyhow::Result;
use dashmap::DashMap;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

const FLOW_CACHE_TTL: Duration = Duration::from_secs(120);
const TCP_SYN_FLAG: u8 = 0x02;
const TCP_ACK_FLAG: u8 = 0x10;
const TCP_FIN_FLAG: u8 = 0x01;
const TCP_RST_FLAG: u8 = 0x04;

#[derive(Clone)]
struct CachedFlowDecision {
    decision: RouteDecision,
    pid: Option<u32>,
    process_name: String,
    exe_path: Option<String>,
    process_label: String,
    intercepted_tcp: bool,
    last_seen: Instant,
}

/// Hot-swappable immutable snapshot of rules and upstream proxies.
pub(crate) struct RuntimeConfig {
    pub(crate) config: Config,
    pub(crate) rules: Arc<RuleEngine>,
    pub(crate) proxies: Arc<HashMap<String, ProxyConfig>>,
}

/// Live control handle for inspecting telemetry and hot-reloading rules/proxies on a running engine.
#[derive(Clone)]
pub struct EngineHandle {
    capture: Arc<dyn CaptureEngine>,
    runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
    flow_cache: Arc<DashMap<FiveTuple, CachedFlowDecision>>,
    enable_sniffing: Arc<AtomicBool>,
    block_quic_when_no_udp: Arc<AtomicBool>,
    telemetry: Arc<TelemetryHub>,
}

impl EngineHandle {
    pub fn telemetry(&self) -> Arc<TelemetryHub> {
        Arc::clone(&self.telemetry)
    }

    pub fn config_snapshot(&self) -> Config {
        self.runtime
            .read()
            .expect("runtime config lock poisoned")
            .config
            .clone()
    }

    /// Validates and atomically hot-reloads rules, proxies, and general flags without
    /// dropping the capture handle or breaking active TCP connections.
    pub fn reload_config(&self, new_cfg: Config) -> Result<()> {
        let built = build_runtime_config(new_cfg, self.capture.as_ref())?;
        let rules_count = built.config.rules.len();
        let proxies_count = built.config.proxies.len();

        self.enable_sniffing
            .store(built.config.general.enable_sniffing, Ordering::Relaxed);
        self.block_quic_when_no_udp.store(
            built.config.general.block_quic_when_no_udp,
            Ordering::Relaxed,
        );

        {
            let mut guard = self.runtime.write().expect("runtime config lock poisoned");
            *guard = Arc::new(built);
        }

        // Retain currently intercepted TCP flows so active sockets stay intact, while clearing
        // cached Direct/Block/UDP decisions so updated rules take effect immediately.
        self.flow_cache.retain(|_, v| v.intercepted_tcp);

        info!(
            rules = rules_count,
            proxies = proxies_count,
            "hot-reloaded bypath rules and proxies"
        );
        self.telemetry
            .notify_config_reloaded(rules_count, proxies_count);
        Ok(())
    }
}

/// Shared context passed to packet dispatchers and async PID-retry tasks.
#[derive(Clone)]
struct EngineCtx {
    capture: Arc<dyn CaptureEngine>,
    tracker: Arc<dyn ProcessTracker>,
    runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
    flow_cache: Arc<DashMap<FiveTuple, CachedFlowDecision>>,
    netstack: Arc<TcpNetstack>,
    udp_relay: Arc<UdpRelay>,
    telemetry: Arc<TelemetryHub>,
}

/// Top-level transparent proxy engine coordinating capture, rules, netstack, UDP relay, and telemetry.
pub struct ProxyEngine {
    capture: Arc<dyn CaptureEngine>,
    tracker: Arc<dyn ProcessTracker>,
    runtime: Arc<RwLock<Arc<RuntimeConfig>>>,
    flow_cache: Arc<DashMap<FiveTuple, CachedFlowDecision>>,
    enable_sniffing: Arc<AtomicBool>,
    block_quic_when_no_udp: Arc<AtomicBool>,
    telemetry: Arc<TelemetryHub>,
}

impl ProxyEngine {
    pub fn new(
        config: Config,
        capture: Arc<dyn CaptureEngine>,
        tracker: Arc<dyn ProcessTracker>,
    ) -> Result<Self> {
        Self::with_telemetry(config, capture, tracker, TelemetryHub::new())
    }

    pub fn with_telemetry(
        config: Config,
        capture: Arc<dyn CaptureEngine>,
        tracker: Arc<dyn ProcessTracker>,
        telemetry: Arc<TelemetryHub>,
    ) -> Result<Self> {
        let built = build_runtime_config(config, capture.as_ref())?;
        let enable_sniffing = Arc::new(AtomicBool::new(built.config.general.enable_sniffing));
        let block_quic_when_no_udp = Arc::new(AtomicBool::new(
            built.config.general.block_quic_when_no_udp,
        ));
        Ok(Self {
            capture,
            tracker,
            runtime: Arc::new(RwLock::new(Arc::new(built))),
            flow_cache: Arc::new(DashMap::new()),
            enable_sniffing,
            block_quic_when_no_udp,
            telemetry,
        })
    }

    /// Returns a cloneable control handle for hot-reload and live telemetry inspection.
    pub fn handle(&self) -> EngineHandle {
        EngineHandle {
            capture: Arc::clone(&self.capture),
            runtime: Arc::clone(&self.runtime),
            flow_cache: Arc::clone(&self.flow_cache),
            enable_sniffing: Arc::clone(&self.enable_sniffing),
            block_quic_when_no_udp: Arc::clone(&self.block_quic_when_no_udp),
            telemetry: Arc::clone(&self.telemetry),
        }
    }

    /// Runs the capture and forwarding loop until `shutdown_rx` resolves or the capture stops.
    pub async fn run(self, mut shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> Result<()> {
        let (pkt_tx, mut pkt_rx) = mpsc::channel::<CapturedPacket>(4096);
        let (netstack, netstack_handle) = TcpNetstack::spawn(
            Arc::clone(&self.capture),
            Arc::clone(&self.tracker),
            Arc::clone(&self.runtime),
            Arc::clone(&self.enable_sniffing),
            Arc::clone(&self.telemetry),
        );
        let udp_relay = Arc::new(UdpRelay::new(
            Arc::clone(&self.capture),
            Arc::clone(&self.tracker),
            Arc::clone(&self.runtime),
            Arc::clone(&self.block_quic_when_no_udp),
            Arc::clone(&self.telemetry),
        ));

        let ctx = EngineCtx {
            capture: Arc::clone(&self.capture),
            tracker: Arc::clone(&self.tracker),
            runtime: Arc::clone(&self.runtime),
            flow_cache: Arc::clone(&self.flow_cache),
            netstack: Arc::new(netstack),
            udp_relay,
            telemetry: Arc::clone(&self.telemetry),
        };

        if let Err(err) = self.capture.start(pkt_tx) {
            self.telemetry.set_running(false, Some(err.to_string()));
            return Err(err);
        }

        self.telemetry.set_running(true, None);
        info!("bypath engine started");

        let mut gc_interval = tokio::time::interval(Duration::from_secs(30));
        let mut stats_interval = tokio::time::interval(Duration::from_secs(1));

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    info!("shutdown signal received, releasing capture handles");
                    break;
                }
                _ = stats_interval.tick() => {
                    self.telemetry.sample_rates_and_emit();
                }
                _ = gc_interval.tick() => {
                    let now = Instant::now();
                    self.flow_cache.retain(|_, v| now.duration_since(v.last_seen) < FLOW_CACHE_TTL);
                    let rt = Arc::clone(&self.runtime.read().expect("runtime config lock poisoned"));
                    refresh_exempt_proxy_pids(&rt.config, &rt.rules, self.capture.as_ref());
                }
                maybe_pkt = pkt_rx.recv() => {
                    match maybe_pkt {
                        Some(pkt) => {
                            dispatch_packet(&ctx, pkt);
                        }
                        None => {
                            warn!("capture packet channel closed");
                            break;
                        }
                    }
                }
            }
        }

        let _ = self.capture.shutdown();
        self.telemetry.set_running(false, None);
        drop(ctx);
        let _ = netstack_handle.await;
        Ok(())
    }
}

fn build_runtime_config(config: Config, capture: &dyn CaptureEngine) -> Result<RuntimeConfig> {
    config.validate()?;
    let rules = Arc::new(RuleEngine::from_config(&config)?);
    refresh_exempt_proxy_pids(&config, &rules, capture);

    let proxies = Arc::new(config.proxies.clone());
    Ok(RuntimeConfig {
        config,
        rules,
        proxies,
    })
}

fn refresh_exempt_proxy_pids(config: &Config, rules: &RuleEngine, capture: &dyn CaptureEngine) {
    // Auto-discover and exempt local upstream proxy listener PIDs to prevent routing loops,
    // including when the user restarts their local proxy client at runtime.
    for (name, proxy) in &config.proxies {
        if let Ok(addr) = proxy.parse_socket_addr() {
            if addr.ip().is_loopback() {
                if let Some(pid) = capture.find_local_tcp_listener_pid(addr.port()) {
                    if rules.add_exempt_pid(pid) {
                        info!(
                            proxy = %name,
                            address = %addr,
                            pid,
                            "auto-exempted local upstream proxy PID (anti-loop)"
                        );
                    }
                }
            }
        }
    }
}

fn dispatch_packet(ctx: &EngineCtx, pkt: CapturedPacket) {
    if !pkt.meta.outbound {
        let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
        return;
    }

    let Some(parsed) = parse_ip_packet(&pkt.payload) else {
        let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
        return;
    };

    let tuple = parsed.tuple;
    let is_syn_only = tuple.protocol == TransportProtocol::Tcp
        && (parsed.tcp_flags & TCP_SYN_FLAG) != 0
        && (parsed.tcp_flags & TCP_ACK_FLAG) == 0;
    let is_tcp_close = tuple.protocol == TransportProtocol::Tcp
        && (parsed.tcp_flags & (TCP_FIN_FLAG | TCP_RST_FLAG)) != 0;

    // Fast path: check cached flow verdict
    if !is_syn_only {
        if let Some(mut entry) = ctx.flow_cache.get_mut(&tuple) {
            entry.last_seen = Instant::now();
            let cloned = entry.clone();
            drop(entry);
            apply_decision(
                ctx,
                pkt,
                tuple,
                parsed.l4_payload_offset,
                is_syn_only,
                is_tcp_close,
                cloned,
                false,
            );
            return;
        }

        // Non-SYN TCP packet belonging to a connection established before Bypath started:
        // pass straight through untouched so pre-existing sockets are never disrupted.
        if tuple.protocol == TransportProtocol::Tcp {
            ctx.telemetry.record_direct_packet();
            let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
            return;
        }
    }

    // Immediate non-blocking PID lookup (SOCKET cache + IP Helper table)
    if let Some(pid) = ctx.capture.lookup_pid(&tuple) {
        let entry = classify_and_cache(ctx, Some(pid), &tuple);
        apply_decision(
            ctx,
            pkt,
            tuple,
            parsed.l4_payload_offset,
            is_syn_only,
            is_tcp_close,
            entry,
            true,
        );
        return;
    }

    // Rare first-packet race: offload the micro-retry to a background task so the
    // main capture loop NEVER suffers head-of-line blocking for unrelated system traffic.
    let ctx_clone = ctx.clone();
    let payload_offset = parsed.l4_payload_offset;
    tokio::spawn(async move {
        let mut resolved = None;
        for delay_ms in [1u64, 4] {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            if let Some(pid) = ctx_clone.capture.lookup_pid(&tuple) {
                resolved = Some(pid);
                break;
            }
        }
        let entry = classify_and_cache(&ctx_clone, resolved, &tuple);
        apply_decision(
            &ctx_clone,
            pkt,
            tuple,
            payload_offset,
            is_syn_only,
            is_tcp_close,
            entry,
            true,
        );
    });
}

fn classify_and_cache(
    ctx: &EngineCtx,
    pid: Option<u32>,
    tuple: &FiveTuple,
) -> CachedFlowDecision {
    let proc_info = pid.and_then(|p| ctx.tracker.get_process(p));
    let (process_name, exe_path, process_label) = match proc_info {
        Some(info) => {
            let label = format!("{}({})", info.name, info.pid);
            (info.name, info.exe_path, label)
        }
        None => {
            let fallback = pid
                .map(|p| format!("pid:{p}"))
                .unwrap_or_else(|| "unknown".to_string());
            (fallback.clone(), None, fallback)
        }
    };

    let rt = Arc::clone(&ctx.runtime.read().expect("runtime config lock poisoned"));
    let decision = rt.rules.evaluate(pid, tuple, ctx.tracker.as_ref());
    let intercepted_tcp = tuple.protocol == TransportProtocol::Tcp
        && matches!(
            decision,
            RouteDecision::Proxy { .. } | RouteDecision::InspectDirect { .. }
        );

    let entry = CachedFlowDecision {
        decision,
        pid,
        process_name,
        exe_path,
        process_label,
        intercepted_tcp,
        last_seen: Instant::now(),
    };
    ctx.flow_cache.insert(*tuple, entry.clone());
    entry
}

#[allow(clippy::too_many_arguments)]
fn apply_decision(
    ctx: &EngineCtx,
    pkt: CapturedPacket,
    tuple: FiveTuple,
    l4_payload_offset: usize,
    is_syn_only: bool,
    is_tcp_close: bool,
    entry: CachedFlowDecision,
    is_new_classification: bool,
) {
    match entry.decision {
        RouteDecision::Direct { .. } => {
            ctx.telemetry.record_direct_packet();
            let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
        }
        RouteDecision::Block { ref rule_name } => {
            debug!(
                proc = %entry.process_label,
                tuple = ?tuple,
                rule = %rule_name,
                "blocked packet by rule"
            );
            if is_new_classification {
                ctx.telemetry.record_blocked(
                    NewConnectionParams {
                        pid: entry.pid,
                        process_name: entry.process_name,
                        exe_path: entry.exe_path,
                        protocol: match tuple.protocol {
                            TransportProtocol::Tcp => ConnectionProtocol::Tcp,
                            TransportProtocol::Udp => ConnectionProtocol::Udp,
                        },
                        src_addr: SocketAddr::new(tuple.src_ip, tuple.src_port).to_string(),
                        dst_ip: tuple.dst_ip.to_string(),
                        dst_port: tuple.dst_port,
                        rule_name: rule_name.clone(),
                        proxy_name: None,
                        action: ConnectionAction::Block,
                    },
                    Some(format!("Blocked by rule '{rule_name}'")),
                );
            }
        }
        RouteDecision::InspectDirect { ref rule_name } => {
            if tuple.protocol != TransportProtocol::Tcp || !entry.intercepted_tcp {
                ctx.telemetry.record_direct_packet();
                let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
                return;
            }
            let input = NetstackInput {
                packet: pkt.payload,
                tuple,
                meta: pkt.meta,
                proxy: None,
                proxy_name: None,
                rule_name: rule_name.clone(),
                pid: entry.pid,
                process_name: entry.process_name,
                exe_path: entry.exe_path,
                process_label: entry.process_label,
                is_syn_only,
            };
            if let Err(err) = ctx.netstack.try_feed(input) {
                warn!(
                    ?err,
                    "netstack input channel full; dropping TCP segment for retransmit"
                );
            }
            if is_tcp_close {
                if let Some(mut e) = ctx.flow_cache.get_mut(&tuple) {
                    e.last_seen = Instant::now();
                }
            }
        }
        RouteDecision::Proxy {
            ref rule_name,
            ref proxy_name,
        } => {
            let rt = Arc::clone(&ctx.runtime.read().expect("runtime config lock poisoned"));
            let Some(proxy) = rt.proxies.get(proxy_name).cloned() else {
                let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
                return;
            };

            match tuple.protocol {
                TransportProtocol::Tcp => {
                    if !entry.intercepted_tcp {
                        let _ = ctx.capture.inject(&pkt.payload, &pkt.meta);
                        return;
                    }
                    let input = NetstackInput {
                        packet: pkt.payload,
                        tuple,
                        meta: pkt.meta,
                        proxy: Some(proxy),
                        proxy_name: Some(proxy_name.clone()),
                        rule_name: rule_name.clone(),
                        pid: entry.pid,
                        process_name: entry.process_name,
                        exe_path: entry.exe_path,
                        process_label: entry.process_label,
                        is_syn_only,
                    };
                    if let Err(err) = ctx.netstack.try_feed(input) {
                        warn!(
                            ?err,
                            "netstack input channel full; dropping TCP segment for retransmit"
                        );
                    }
                    if is_tcp_close {
                        if let Some(mut e) = ctx.flow_cache.get_mut(&tuple) {
                            e.last_seen = Instant::now();
                        }
                    }
                }
                TransportProtocol::Udp => {
                    let udp_payload = pkt.payload[l4_payload_offset..].to_vec();
                    let udp_relay = Arc::clone(&ctx.udp_relay);
                    let udp_ctx = UdpPacketContext {
                        pid: entry.pid,
                        process_name: entry.process_name,
                        exe_path: entry.exe_path,
                        process_label: entry.process_label,
                        rule_name: rule_name.clone(),
                        proxy_name: proxy_name.clone(),
                        proxy,
                    };
                    tokio::spawn(async move {
                        udp_relay
                            .handle_outbound_udp(tuple, udp_payload, pkt.meta, udp_ctx)
                            .await;
                    });
                }
            }
        }
    }
}
