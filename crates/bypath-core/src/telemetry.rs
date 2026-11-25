use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;

const MAX_RETAINED_CONNECTIONS: usize = 500;
const BYTE_BROADCAST_INTERVAL_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionProtocol {
    Tcp,
    Udp,
    Quic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionAction {
    Proxy,
    Direct,
    Block,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionState {
    Active,
    Closed,
    Failed,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionRecord {
    pub id: u64,
    pub pid: Option<u32>,
    pub process_name: String,
    pub exe_path: Option<String>,
    pub protocol: ConnectionProtocol,
    pub src_addr: String,
    pub dst_ip: String,
    pub dst_port: u16,
    pub sniffed_host: Option<String>,
    pub sniff_kind: Option<String>,
    pub rule_name: String,
    pub proxy_name: Option<String>,
    pub action: ConnectionAction,
    pub state: ConnectionState,
    pub bytes_tx: u64,
    pub bytes_rx: u64,
    pub started_at_ms: u64,
    pub updated_at_ms: u64,
    pub ended_at_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineStats {
    pub running: bool,
    pub uptime_secs: u64,
    pub active_connections: u64,
    pub total_connections: u64,
    pub proxied_connections: u64,
    pub blocked_connections: u64,
    pub direct_packets: u64,
    pub total_bytes_tx: u64,
    pub total_bytes_rx: u64,
    pub tx_bytes_per_sec: u64,
    pub rx_bytes_per_sec: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    StatsTick {
        stats: EngineStats,
    },
    ConnectionUpsert {
        connection: ConnectionRecord,
    },
    ConnectionsCleared,
    ConfigReloaded {
        rules_count: usize,
        proxies_count: usize,
    },
    StateChanged {
        running: bool,
        error: Option<String>,
    },
}

struct StoredConnection {
    record: ConnectionRecord,
    last_broadcast_ms: u64,
}

#[derive(Default)]
struct RateState {
    started_at: Option<Instant>,
    last_sample_at: Option<Instant>,
    last_bytes_tx: u64,
    last_bytes_rx: u64,
    tx_rate: u64,
    rx_rate: u64,
}

/// Parameters required when registering a newly intercepted or blocked connection.
pub struct NewConnectionParams {
    pub pid: Option<u32>,
    pub process_name: String,
    pub exe_path: Option<String>,
    pub protocol: ConnectionProtocol,
    pub src_addr: String,
    pub dst_ip: String,
    pub dst_port: u16,
    pub rule_name: String,
    pub proxy_name: Option<String>,
    pub action: ConnectionAction,
}

/// Thread-safe live telemetry store and broadcast hub shared between the engine, IPC, and GUI.
pub struct TelemetryHub {
    next_id: AtomicU64,
    running: AtomicBool,
    active_connections: AtomicU64,
    total_connections: AtomicU64,
    proxied_connections: AtomicU64,
    blocked_connections: AtomicU64,
    direct_packets: AtomicU64,
    total_bytes_tx: AtomicU64,
    total_bytes_rx: AtomicU64,
    rate_state: RwLock<RateState>,
    connections: RwLock<VecDeque<StoredConnection>>,
    events_tx: broadcast::Sender<EngineEvent>,
}

impl Default for TelemetryHub {
    fn default() -> Self {
        let (events_tx, _) = broadcast::channel(512);
        Self {
            next_id: AtomicU64::new(1),
            running: AtomicBool::new(false),
            active_connections: AtomicU64::new(0),
            total_connections: AtomicU64::new(0),
            proxied_connections: AtomicU64::new(0),
            blocked_connections: AtomicU64::new(0),
            direct_packets: AtomicU64::new(0),
            total_bytes_tx: AtomicU64::new(0),
            total_bytes_rx: AtomicU64::new(0),
            rate_state: RwLock::new(RateState::default()),
            connections: RwLock::new(VecDeque::with_capacity(MAX_RETAINED_CONNECTIONS)),
            events_tx,
        }
    }
}

impl TelemetryHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events_tx.subscribe()
    }

    pub fn set_running(&self, running: bool, error: Option<String>) {
        self.running.store(running, Ordering::SeqCst);
        if let Ok(mut rs) = self.rate_state.write() {
            if running {
                let now = Instant::now();
                rs.started_at = Some(now);
                rs.last_sample_at = Some(now);
                rs.last_bytes_tx = self.total_bytes_tx.load(Ordering::Relaxed);
                rs.last_bytes_rx = self.total_bytes_rx.load(Ordering::Relaxed);
                rs.tx_rate = 0;
                rs.rx_rate = 0;
            } else {
                rs.started_at = None;
                rs.tx_rate = 0;
                rs.rx_rate = 0;
                // Mark any still-active connections as closed cleanly when engine stops.
                self.close_all_active();
            }
        }
        let _ = self
            .events_tx
            .send(EngineEvent::StateChanged { running, error });
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn open_connection(&self, params: NewConnectionParams) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now_ms = unix_timestamp_ms();
        let record = ConnectionRecord {
            id,
            pid: params.pid,
            process_name: params.process_name,
            exe_path: params.exe_path,
            protocol: params.protocol,
            src_addr: params.src_addr,
            dst_ip: params.dst_ip,
            dst_port: params.dst_port,
            sniffed_host: None,
            sniff_kind: None,
            rule_name: params.rule_name,
            proxy_name: params.proxy_name,
            action: params.action,
            state: ConnectionState::Active,
            bytes_tx: 0,
            bytes_rx: 0,
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
            ended_at_ms: None,
            error: None,
        };

        self.active_connections.fetch_add(1, Ordering::Relaxed);
        self.total_connections.fetch_add(1, Ordering::Relaxed);
        if params.action == ConnectionAction::Proxy {
            self.proxied_connections.fetch_add(1, Ordering::Relaxed);
        }

        if let Ok(mut list) = self.connections.write() {
            list.push_front(StoredConnection {
                record: record.clone(),
                last_broadcast_ms: now_ms,
            });
            prune_connections(&mut list);
        }

        let _ = self
            .events_tx
            .send(EngineEvent::ConnectionUpsert { connection: record });
        id
    }

    pub fn record_blocked(&self, params: NewConnectionParams, reason: Option<String>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now_ms = unix_timestamp_ms();
        let record = ConnectionRecord {
            id,
            pid: params.pid,
            process_name: params.process_name,
            exe_path: params.exe_path,
            protocol: params.protocol,
            src_addr: params.src_addr,
            dst_ip: params.dst_ip,
            dst_port: params.dst_port,
            sniffed_host: None,
            sniff_kind: None,
            rule_name: params.rule_name,
            proxy_name: params.proxy_name,
            action: ConnectionAction::Block,
            state: ConnectionState::Blocked,
            bytes_tx: 0,
            bytes_rx: 0,
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
            ended_at_ms: Some(now_ms),
            error: reason,
        };

        self.total_connections.fetch_add(1, Ordering::Relaxed);
        self.blocked_connections.fetch_add(1, Ordering::Relaxed);

        if let Ok(mut list) = self.connections.write() {
            list.push_front(StoredConnection {
                record: record.clone(),
                last_broadcast_ms: now_ms,
            });
            prune_connections(&mut list);
        }

        let _ = self
            .events_tx
            .send(EngineEvent::ConnectionUpsert { connection: record });
        id
    }

    pub fn update_sniff(&self, id: u64, host: String, kind: &str) {
        let now_ms = unix_timestamp_ms();
        let mut updated = None;
        if let Ok(mut list) = self.connections.write() {
            if let Some(entry) = list.iter_mut().find(|c| c.record.id == id) {
                entry.record.sniffed_host = Some(host);
                entry.record.sniff_kind = Some(kind.to_string());
                entry.record.updated_at_ms = now_ms;
                entry.last_broadcast_ms = now_ms;
                updated = Some(entry.record.clone());
            }
        }
        if let Some(connection) = updated {
            let _ = self
                .events_tx
                .send(EngineEvent::ConnectionUpsert { connection });
        }
    }

    /// Transitions an active connection into `Blocked` after a domain-level rule matches.
    pub fn mark_connection_blocked(
        &self,
        id: u64,
        rule_name: String,
        reason: Option<String>,
    ) {
        let now_ms = unix_timestamp_ms();
        let mut updated = None;
        let mut was_active = false;
        let mut was_proxied = false;

        if let Ok(mut list) = self.connections.write() {
            if let Some(entry) = list.iter_mut().find(|c| c.record.id == id) {
                was_active = entry.record.state == ConnectionState::Active;
                was_proxied = entry.record.action == ConnectionAction::Proxy;
                entry.record.action = ConnectionAction::Block;
                entry.record.state = ConnectionState::Blocked;
                entry.record.rule_name = rule_name;
                entry.record.proxy_name = None;
                entry.record.error = reason;
                entry.record.updated_at_ms = now_ms;
                entry.record.ended_at_ms = Some(now_ms);
                entry.last_broadcast_ms = now_ms;
                updated = Some(entry.record.clone());
            }
        }

        if was_active {
            decrement_saturating(&self.active_connections);
        }
        if was_proxied {
            decrement_saturating(&self.proxied_connections);
        }
        self.blocked_connections.fetch_add(1, Ordering::Relaxed);

        if let Some(connection) = updated {
            let _ = self
                .events_tx
                .send(EngineEvent::ConnectionUpsert { connection });
        }
    }

    /// Updates a connection's effective routing action/proxy after a domain-level rule override.
    pub fn update_connection_route(
        &self,
        id: u64,
        action: ConnectionAction,
        rule_name: String,
        proxy_name: Option<String>,
    ) {
        let now_ms = unix_timestamp_ms();
        let mut updated = None;
        let mut prev_action = None;

        if let Ok(mut list) = self.connections.write() {
            if let Some(entry) = list.iter_mut().find(|c| c.record.id == id) {
                prev_action = Some(entry.record.action);
                entry.record.action = action;
                entry.record.rule_name = rule_name;
                entry.record.proxy_name = proxy_name;
                entry.record.updated_at_ms = now_ms;
                entry.last_broadcast_ms = now_ms;
                updated = Some(entry.record.clone());
            }
        }

        if let Some(prev) = prev_action {
            if prev == ConnectionAction::Proxy && action != ConnectionAction::Proxy {
                decrement_saturating(&self.proxied_connections);
            } else if prev != ConnectionAction::Proxy && action == ConnectionAction::Proxy {
                self.proxied_connections.fetch_add(1, Ordering::Relaxed);
            }
        }

        if let Some(connection) = updated {
            let _ = self
                .events_tx
                .send(EngineEvent::ConnectionUpsert { connection });
        }
    }

    pub fn add_bytes(&self, id: u64, tx_delta: u64, rx_delta: u64) {
        if tx_delta == 0 && rx_delta == 0 {
            return;
        }
        if tx_delta > 0 {
            self.total_bytes_tx.fetch_add(tx_delta, Ordering::Relaxed);
        }
        if rx_delta > 0 {
            self.total_bytes_rx.fetch_add(rx_delta, Ordering::Relaxed);
        }

        let now_ms = unix_timestamp_ms();
        let mut to_broadcast = None;
        if let Ok(mut list) = self.connections.write() {
            if let Some(entry) = list.iter_mut().find(|c| c.record.id == id) {
                entry.record.bytes_tx = entry.record.bytes_tx.saturating_add(tx_delta);
                entry.record.bytes_rx = entry.record.bytes_rx.saturating_add(rx_delta);
                entry.record.updated_at_ms = now_ms;
                if now_ms.saturating_sub(entry.last_broadcast_ms) >= BYTE_BROADCAST_INTERVAL_MS {
                    entry.last_broadcast_ms = now_ms;
                    to_broadcast = Some(entry.record.clone());
                }
            }
        }
        if let Some(connection) = to_broadcast {
            let _ = self
                .events_tx
                .send(EngineEvent::ConnectionUpsert { connection });
        }
    }

    pub fn close_connection(&self, id: u64, error: Option<String>) {
        let now_ms = unix_timestamp_ms();
        let mut updated = None;
        let mut was_active = false;

        if let Ok(mut list) = self.connections.write() {
            if let Some(entry) = list.iter_mut().find(|c| c.record.id == id) {
                if entry.record.state == ConnectionState::Active {
                    was_active = true;
                }
                entry.record.state = if error.is_some() {
                    ConnectionState::Failed
                } else {
                    ConnectionState::Closed
                };
                entry.record.error = error;
                entry.record.updated_at_ms = now_ms;
                entry.record.ended_at_ms = Some(now_ms);
                entry.last_broadcast_ms = now_ms;
                updated = Some(entry.record.clone());
            }
        }

        if was_active {
            decrement_saturating(&self.active_connections);
        }

        if let Some(connection) = updated {
            let _ = self
                .events_tx
                .send(EngineEvent::ConnectionUpsert { connection });
        }
    }

    pub fn record_direct_packet(&self) {
        self.direct_packets.fetch_add(1, Ordering::Relaxed);
    }

    pub fn clear_closed_connections(&self) {
        if let Ok(mut list) = self.connections.write() {
            list.retain(|c| c.record.state == ConnectionState::Active);
        }
        let _ = self.events_tx.send(EngineEvent::ConnectionsCleared);
    }

    pub fn list_connections(&self) -> Vec<ConnectionRecord> {
        self.connections
            .read()
            .ok()
            .map(|list| list.iter().map(|c| c.record.clone()).collect())
            .unwrap_or_default()
    }

    pub fn notify_config_reloaded(&self, rules_count: usize, proxies_count: usize) {
        let _ = self.events_tx.send(EngineEvent::ConfigReloaded {
            rules_count,
            proxies_count,
        });
    }

    /// Computes live TX/RX throughput rates over the elapsed sample window and emits `StatsTick`.
    pub fn sample_rates_and_emit(&self) -> EngineStats {
        let now = Instant::now();
        let cur_tx = self.total_bytes_tx.load(Ordering::Relaxed);
        let cur_rx = self.total_bytes_rx.load(Ordering::Relaxed);

        if let Ok(mut rs) = self.rate_state.write() {
            if let Some(prev) = rs.last_sample_at {
                let elapsed = now.duration_since(prev).as_secs_f64();
                if elapsed >= 0.1 {
                    let dtx = cur_tx.saturating_sub(rs.last_bytes_tx);
                    let drx = cur_rx.saturating_sub(rs.last_bytes_rx);
                    rs.tx_rate = (dtx as f64 / elapsed) as u64;
                    rs.rx_rate = (drx as f64 / elapsed) as u64;
                    rs.last_sample_at = Some(now);
                    rs.last_bytes_tx = cur_tx;
                    rs.last_bytes_rx = cur_rx;
                }
            } else {
                rs.last_sample_at = Some(now);
                rs.last_bytes_tx = cur_tx;
                rs.last_bytes_rx = cur_rx;
            }
        }

        let stats = self.stats_snapshot();
        let _ = self.events_tx.send(EngineEvent::StatsTick {
            stats: stats.clone(),
        });
        stats
    }

    pub fn stats_snapshot(&self) -> EngineStats {
        let (uptime_secs, tx_rate, rx_rate) = self
            .rate_state
            .read()
            .ok()
            .map(|rs| {
                let up = rs
                    .started_at
                    .map(|s| s.elapsed().as_secs())
                    .unwrap_or(0);
                (up, rs.tx_rate, rs.rx_rate)
            })
            .unwrap_or((0, 0, 0));

        EngineStats {
            running: self.running.load(Ordering::SeqCst),
            uptime_secs,
            active_connections: self.active_connections.load(Ordering::Relaxed),
            total_connections: self.total_connections.load(Ordering::Relaxed),
            proxied_connections: self.proxied_connections.load(Ordering::Relaxed),
            blocked_connections: self.blocked_connections.load(Ordering::Relaxed),
            direct_packets: self.direct_packets.load(Ordering::Relaxed),
            total_bytes_tx: self.total_bytes_tx.load(Ordering::Relaxed),
            total_bytes_rx: self.total_bytes_rx.load(Ordering::Relaxed),
            tx_bytes_per_sec: tx_rate,
            rx_bytes_per_sec: rx_rate,
        }
    }

    fn close_all_active(&self) {
        let now_ms = unix_timestamp_ms();
        if let Ok(mut list) = self.connections.write() {
            for entry in list.iter_mut() {
                if entry.record.state == ConnectionState::Active {
                    entry.record.state = ConnectionState::Closed;
                    entry.record.updated_at_ms = now_ms;
                    entry.record.ended_at_ms = Some(now_ms);
                }
            }
        }
        self.active_connections.store(0, Ordering::Relaxed);
    }
}

fn prune_connections(list: &mut VecDeque<StoredConnection>) {
    while list.len() > MAX_RETAINED_CONNECTIONS {
        // Prefer pruning the oldest non-active connection first.
        if let Some(pos) = list
            .iter()
            .rposition(|c| c.record.state != ConnectionState::Active)
        {
            list.remove(pos);
        } else {
            list.pop_back();
        }
    }
}

fn decrement_saturating(counter: &AtomicU64) {
    let mut cur = counter.load(Ordering::Relaxed);
    while cur > 0 {
        match counter.compare_exchange_weak(cur, cur - 1, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(next) => cur = next,
        }
    }
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_lifecycle_tracks_counts_and_bytes() {
        let hub = TelemetryHub::new();
        hub.set_running(true, None);

        let id = hub.open_connection(NewConnectionParams {
            pid: Some(43180),
            process_name: "language_server.exe".into(),
            exe_path: Some(r"C:\Antigravity\language_server.exe".into()),
            protocol: ConnectionProtocol::Tcp,
            src_addr: "192.168.1.5:55000".into(),
            dst_ip: "142.250.190.78".into(),
            dst_port: 443,
            rule_name: "Antigravity".into(),
            proxy_name: Some("local_socks".into()),
            action: ConnectionAction::Proxy,
        });

        hub.update_sniff(id, "daily-cloudcode-pa.googleapis.com".into(), "SNI");
        hub.add_bytes(id, 1024, 4096);

        let stats = hub.stats_snapshot();
        assert!(stats.running);
        assert_eq!(stats.active_connections, 1);
        assert_eq!(stats.proxied_connections, 1);
        assert_eq!(stats.total_bytes_tx, 1024);
        assert_eq!(stats.total_bytes_rx, 4096);

        hub.close_connection(id, None);
        let stats_after = hub.stats_snapshot();
        assert_eq!(stats_after.active_connections, 0);
        assert_eq!(stats_after.total_connections, 1);

        let list = hub.list_connections();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, ConnectionState::Closed);
        assert_eq!(
            list[0].sniffed_host.as_deref(),
            Some("daily-cloudcode-pa.googleapis.com")
        );

        hub.clear_closed_connections();
        assert!(hub.list_connections().is_empty());
    }
}
