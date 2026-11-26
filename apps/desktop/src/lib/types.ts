export const APP_VERSION = "0.4.1";

export type BackendMode = "service" | "embedded";
export type ProxyKind = "socks5" | "http";
export type RuleAction = "proxy" | "direct" | "block";
export type ConnectionProtocol = "tcp" | "udp" | "quic";
export type ConnectionAction = "proxy" | "direct" | "block";
export type ConnectionState = "active" | "closed" | "failed" | "blocked";
export type StateFilter = "all" | "active" | "blocked";
export type SortKey =
  | "time"
  | "transfer"
  | "tx"
  | "rx"
  | "process"
  | "destination"
  | "state"
  | "proto";
export type SortDir = "asc" | "desc";
export type ConnectionsViewMode = "streams" | "summary";

export interface GeneralConfig {
  log_level: string;
  enable_sniffing: boolean;
  block_quic_when_no_udp: boolean;
  bypass_lan: boolean;
  windivert_dir: string;
  minimize_to_tray: boolean;
  auto_start_engine: boolean;
  theme: string;
  auto_check_updates?: boolean;
}

export interface ProxyConfig {
  type: ProxyKind;
  address: string;
  username?: string | null;
  password?: string | null;
  udp: boolean;
}

export interface RuleConfig {
  name: string;
  enabled: boolean;
  process_names: string[];
  path_prefixes: string[];
  path_contains: string[];
  include_children: boolean;
  dst_cidrs: string[];
  dst_ports: number[];
  protocols: string[];
  domains: string[];
  action: RuleAction;
  proxy?: string | null;
}

export interface Config {
  general: GeneralConfig;
  proxies: Record<string, ProxyConfig>;
  rules: RuleConfig[];
}

export interface ConfigSnapshot {
  path: string;
  config: Config;
}

export interface EngineStats {
  running: boolean;
  uptime_secs: number;
  active_connections: number;
  total_connections: number;
  proxied_connections: number;
  blocked_connections: number;
  direct_packets: number;
  total_bytes_tx: number;
  total_bytes_rx: number;
  tx_bytes_per_sec: number;
  rx_bytes_per_sec: number;
}

export interface DaemonStatus {
  backend_mode: BackendMode;
  daemon_pid: number;
  elevated: boolean;
  service_installed: boolean;
  service_running: boolean;
  engine_running: boolean;
  windivert_ready: boolean;
  windivert_dll_path: string | null;
  windivert_version: string | null;
  windivert_error: string | null;
  windivert_error_kind?: string | null;
  portable_mode?: boolean;
  config_path: string;
  active_rules_count: number;
  total_rules_count: number;
  proxies_count: number;
  last_error: string | null;
  stats: EngineStats;
}

export interface ConnectionRecord {
  id: number;
  pid: number | null;
  process_name: string;
  exe_path: string | null;
  protocol: ConnectionProtocol;
  src_addr: string;
  dst_ip: string;
  dst_port: number;
  sniffed_host: string | null;
  sniff_kind: string | null;
  rule_name: string;
  proxy_name: string | null;
  action: ConnectionAction;
  state: ConnectionState;
  bytes_tx: number;
  bytes_rx: number;
  started_at_ms: number;
  updated_at_ms: number;
  ended_at_ms: number | null;
  error: string | null;
}

export interface ProcessEntry {
  pid: number;
  parent_pid: number;
  name: string;
  exe_path: string | null;
  matched_rule: string | null;
}

export interface ProxyProbeReport {
  address: string;
  kind: ProxyKind;
  tcp_reachable: boolean;
  handshake_ok?: boolean | null;
  auth_ok?: boolean | null;
  latency_ms: number | null;
  udp_associate_supported: boolean | null;
  error: string | null;
}

export interface NamedProxyProbe {
  name: string;
  report: ProxyProbeReport;
  local_listener_pid: number | null;
}

export interface DoctorReport {
  elevated: boolean;
  windivert_dll_loaded: boolean;
  windivert_dll_path: string | null;
  windivert_driver_opened: boolean;
  windivert_version: string | null;
  windivert_error: string | null;
  windivert_error_kind?: string | null;
  proxies: NamedProxyProbe[];
}

export interface SystemProxyInfo {
  enabled: boolean;
  source: string;
  raw_server: string | null;
  socks_address: string | null;
  http_address: string | null;
  bypass_list: string | null;
  suggested_kind: ProxyKind | null;
  suggested_address: string | null;
}

export interface RuleTestQuery {
  process_name?: string | null;
  exe_path?: string | null;
  host?: string | null;
  dst_ip?: string | null;
  dst_port: number;
  protocol: string;
}

export interface RuleTestReport {
  phase1_decision: string;
  phase1_rule: string | null;
  phase1_reason: string;
  phase2_decision: string | null;
  phase2_rule: string | null;
  final_action: string;
  final_proxy: string | null;
  matched_rule: string | null;
}

export interface LogEntry {
  id: number;
  timestamp_ms: number;
  level: string;
  target: string;
  message: string;
}

export interface AgentInfo {
  cli_path: string;
  pipe_name: string;
  config_path: string;
  windivert_dir: string;
  portable_mode: boolean;
}

export interface TrayStatePayload {
  tooltip: string;
  minimize_to_tray?: boolean;
}

export interface UiPrefsSyncPayload {
  locale?: string | null;
  theme?: string | null;
}

export interface PresetInfo {
  id: string;
  name: string;
  category: string;
  description: string;
  process_names: string[];
  path_contains: string[];
  include_children: boolean;
}

export interface ServiceStateSummary {
  installed: boolean;
  running: boolean;
  state: string;
}

export interface UpdateCheckReport {
  current_version: string;
  latest_version: string;
  latest_tag: string;
  update_available: boolean;
  release_name: string;
  release_url: string;
  published_at: string | null;
  release_notes: string | null;
  primary_asset_name: string | null;
  primary_asset_url: string | null;
  primary_asset_size: number | null;
  nodriver_asset_name: string | null;
  nodriver_asset_url: string | null;
  checked_via: string;
  checked_at_ms: number;
}

export interface ThroughputPoint {
  tx: number;
  rx: number;
}

export interface ConnectionsViewState {
  selectedProcess: string;
  query: string;
  filter: StateFilter;
  sortKey: SortKey;
  sortDir: SortDir;
  pageSize: number;
  page: number;
  expandedId: number | null;
  viewMode: ConnectionsViewMode;
  summaryGroup: "process" | "domain";
}

export type EngineEvent =
  | { type: "stats_tick"; stats: EngineStats }
  | { type: "connection_upsert"; connection: ConnectionRecord }
  | { type: "connections_cleared" }
  | { type: "config_reloaded"; rules_count: number; proxies_count: number }
  | { type: "state_changed"; running: boolean; error: string | null }
  | { type: "log_entry"; entry: LogEntry };

export type NavTab = "connections" | "applications" | "proxies" | "diagnostics";
