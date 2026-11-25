import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Config,
  ConfigSnapshot,
  ConnectionRecord,
  DaemonStatus,
  DoctorReport,
  EngineEvent,
  NamedProxyProbe,
  PresetInfo,
  ProcessEntry,
  ProxyConfig,
  ServiceStateSummary,
} from "./types";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// Browser preview state used exclusively when opening Vite directly outside Tauri.
const previewPresets: PresetInfo[] = [
  {
    id: "antigravity",
    name: "Antigravity",
    category: "AI & IDE",
    description:
      "Full Electron shell, language_server.exe, and spawned MCP/CLI child processes.",
    process_names: [
      "Antigravity.exe",
      "language_server.exe",
      "language_server_windows_x64.exe",
      "ls_core.exe",
    ],
    path_contains: ["Antigravity"],
    include_children: true,
  },
  {
    id: "cursor",
    name: "Cursor IDE",
    category: "AI & IDE",
    description:
      "Cursor editor, extension host, and spawned AI/language server subprocesses.",
    process_names: ["Cursor.exe"],
    path_contains: ["cursor"],
    include_children: true,
  },
  {
    id: "vscode",
    name: "VS Code",
    category: "AI & IDE",
    description:
      "Visual Studio Code, Copilot/extension hosts, and remote CLI workers.",
    process_names: ["Code.exe"],
    path_contains: ["Microsoft VS Code"],
    include_children: true,
  },
  {
    id: "git",
    name: "Git & SSH CLI",
    category: "Developer",
    description: "Git, git-remote-https, OpenSSH, and curl CLI operations.",
    process_names: ["git.exe", "git-remote-https.exe", "ssh.exe", "curl.exe"],
    path_contains: [],
    include_children: true,
  },
  {
    id: "discord",
    name: "Discord",
    category: "Messaging",
    description: "Discord client, updater, and voice/gateway subprocesses.",
    process_names: ["Discord.exe", "Update.exe"],
    path_contains: ["Discord"],
    include_children: true,
  },
  {
    id: "telegram",
    name: "Telegram Desktop",
    category: "Messaging",
    description: "Telegram Desktop native client.",
    process_names: ["Telegram.exe"],
    path_contains: ["Telegram"],
    include_children: true,
  },
  {
    id: "steam",
    name: "Steam",
    category: "Gaming",
    description: "Steam client, steamwebhelper, and launcher subprocesses.",
    process_names: ["steam.exe", "steamwebhelper.exe"],
    path_contains: ["Steam"],
    include_children: true,
  },
];

let previewConfig: Config = {
  general: {
    log_level: "info",
    enable_sniffing: true,
    block_quic_when_no_udp: true,
    bypass_lan: true,
    windivert_dir: "runtime/windivert",
  },
  proxies: {
    local_socks: {
      type: "socks5",
      address: "127.0.0.1:10808",
      username: null,
      password: null,
      udp: true,
    },
  },
  rules: [
    {
      name: "Preset: Antigravity",
      enabled: true,
      process_names: [
        "Antigravity.exe",
        "language_server.exe",
        "language_server_windows_x64.exe",
      ],
      path_prefixes: [],
      path_contains: ["Antigravity"],
      include_children: true,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains: [],
      action: "proxy",
      proxy: "local_socks",
    },
    {
      name: "Blocked Domains",
      enabled: true,
      process_names: [],
      path_prefixes: [],
      path_contains: [],
      include_children: true,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains: ["*.doubleclick.net", "play.googleapis.com"],
      action: "block",
      proxy: null,
    },
  ],
};

let previewRunning = true;
const now = Date.now();
let previewConnections: ConnectionRecord[] = [
  {
    id: 32,
    pid: 43180,
    process_name: "language_server.exe",
    exe_path:
      "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
    protocol: "tcp",
    src_addr: "192.168.1.24:58412",
    dst_ip: "142.250.199.42",
    dst_port: 443,
    sniffed_host: "daily-cloudcode-pa.googleapis.com",
    sniff_kind: "SNI",
    rule_name: "Preset: Antigravity",
    proxy_name: "local_socks",
    action: "proxy",
    state: "active",
    bytes_tx: 48_320,
    bytes_rx: 612_480,
    started_at_ms: now - 12_000,
    updated_at_ms: now - 400,
    ended_at_ms: null,
    error: null,
  },
  {
    id: 31,
    pid: 43180,
    process_name: "language_server.exe",
    exe_path:
      "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
    protocol: "tcp",
    src_addr: "192.168.1.24:58398",
    dst_ip: "172.217.160.106",
    dst_port: 443,
    sniffed_host: "aicode.googleapis.com",
    sniff_kind: "SNI",
    rule_name: "Preset: Antigravity",
    proxy_name: "local_socks",
    action: "proxy",
    state: "active",
    bytes_tx: 19_840,
    bytes_rx: 184_900,
    started_at_ms: now - 28_000,
    updated_at_ms: now - 900,
    ended_at_ms: null,
    error: null,
  },
  {
    id: 30,
    pid: 43180,
    process_name: "language_server.exe",
    exe_path:
      "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
    protocol: "tcp",
    src_addr: "192.168.1.24:58355",
    dst_ip: "142.250.71.110",
    dst_port: 443,
    sniffed_host: "play.googleapis.com",
    sniff_kind: "SNI",
    rule_name: "Blocked Domains",
    proxy_name: null,
    action: "block",
    state: "blocked",
    bytes_tx: 0,
    bytes_rx: 0,
    started_at_ms: now - 34_000,
    updated_at_ms: now - 34_000,
    ended_at_ms: now - 34_000,
    error: "Blocked domain 'play.googleapis.com' by rule 'Blocked Domains'",
  },
  {
    id: 29,
    pid: 31204,
    process_name: "Antigravity.exe",
    exe_path:
      "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\Antigravity.exe",
    protocol: "tcp",
    src_addr: "192.168.1.24:58290",
    dst_ip: "142.250.71.131",
    dst_port: 443,
    sniffed_host: "www.gstatic.com",
    sniff_kind: "SNI",
    rule_name: "Preset: Antigravity",
    proxy_name: "local_socks",
    action: "proxy",
    state: "active",
    bytes_tx: 8_310,
    bytes_rx: 296_540,
    started_at_ms: now - 52_000,
    updated_at_ms: now - 1_500,
    ended_at_ms: null,
    error: null,
  },
  {
    id: 28,
    pid: 18940,
    process_name: "git-remote-https.exe",
    exe_path: "C:\\Program Files\\Git\\mingw64\\libexec\\git-core\\git-remote-https.exe",
    protocol: "tcp",
    src_addr: "192.168.1.24:58102",
    dst_ip: "140.82.121.4",
    dst_port: 443,
    sniffed_host: "github.com",
    sniff_kind: "SNI",
    rule_name: "Preset: Git & SSH CLI",
    proxy_name: "local_socks",
    action: "proxy",
    state: "closed",
    bytes_tx: 124_400,
    bytes_rx: 1_842_000,
    started_at_ms: now - 95_000,
    updated_at_ms: now - 82_000,
    ended_at_ms: now - 82_000,
    error: null,
  },
  {
    id: 27,
    pid: 43180,
    process_name: "language_server.exe",
    exe_path:
      "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
    protocol: "tcp",
    src_addr: "192.168.1.24:58011",
    dst_ip: "142.250.66.202",
    dst_port: 443,
    sniffed_host: "oauth2.googleapis.com",
    sniff_kind: "SNI",
    rule_name: "Preset: Antigravity",
    proxy_name: "local_socks",
    action: "proxy",
    state: "closed",
    bytes_tx: 4_120,
    bytes_rx: 11_480,
    started_at_ms: now - 115_000,
    updated_at_ms: now - 110_000,
    ended_at_ms: now - 110_000,
    error: null,
  },
];

export const api = {
  async getStatus(): Promise<DaemonStatus> {
    if (isTauriRuntime()) {
      return invoke<DaemonStatus>("get_status");
    }
    const active = previewConnections.filter((c) => c.state === "active").length;
    const blocked = previewConnections.filter((c) => c.state === "blocked").length;
    return {
      backend_mode: "service",
      daemon_pid: 18420,
      elevated: true,
      service_installed: true,
      service_running: true,
      engine_running: previewRunning,
      windivert_ready: true,
      windivert_dll_path: "runtime/windivert\\WinDivert.dll",
      windivert_version: "v2.2",
      windivert_error: null,
      config_path: "C:\\ProgramData\\Bypath\\config.toml",
      active_rules_count: previewConfig.rules.filter((r) => r.enabled).length,
      total_rules_count: previewConfig.rules.length,
      proxies_count: Object.keys(previewConfig.proxies).length,
      last_error: null,
      stats: {
        running: previewRunning,
        uptime_secs: previewRunning ? 842 : 0,
        active_connections: previewRunning ? active : 0,
        total_connections: previewConnections.length,
        proxied_connections: previewConnections.length - blocked,
        blocked_connections: blocked,
        direct_packets: 12_480,
        total_bytes_tx: 204_990,
        total_bytes_rx: 2_947_400,
        tx_bytes_per_sec: previewRunning ? 6_420 : 0,
        rx_bytes_per_sec: previewRunning ? 48_960 : 0,
      },
    };
  },

  async getConfig(): Promise<ConfigSnapshot> {
    if (isTauriRuntime()) {
      return invoke<ConfigSnapshot>("get_config");
    }
    return {
      path: "C:\\ProgramData\\Bypath\\config.toml",
      config: structuredClone(previewConfig),
    };
  },

  async saveConfig(config: Config, reload = true): Promise<string> {
    if (isTauriRuntime()) {
      return invoke<string>("save_config", { config, reload });
    }
    previewConfig = structuredClone(config);
    return "configuration saved";
  },

  async startEngine(): Promise<string> {
    if (isTauriRuntime()) {
      return invoke<string>("start_engine");
    }
    previewRunning = true;
    return "engine started";
  },

  async stopEngine(): Promise<string> {
    if (isTauriRuntime()) {
      return invoke<string>("stop_engine");
    }
    previewRunning = false;
    return "engine stopped";
  },

  async getConnections(): Promise<ConnectionRecord[]> {
    if (isTauriRuntime()) {
      return invoke<ConnectionRecord[]>("get_connections");
    }
    return structuredClone(previewConnections);
  },

  async clearConnections(): Promise<string> {
    if (isTauriRuntime()) {
      return invoke<string>("clear_connections");
    }
    previewConnections = previewConnections.filter((c) => c.state === "active");
    return "closed connections cleared";
  },

  async listProcesses(filter?: string): Promise<ProcessEntry[]> {
    if (isTauriRuntime()) {
      return invoke<ProcessEntry[]>("list_processes", {
        filter: filter?.trim() ? filter : null,
      });
    }
    const sample: ProcessEntry[] = [
      {
        pid: 31204,
        parent_pid: 4120,
        name: "Antigravity.exe",
        exe_path:
          "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\Antigravity.exe",
        matched_rule: "Preset: Antigravity",
      },
      {
        pid: 43180,
        parent_pid: 31204,
        name: "language_server.exe",
        exe_path:
          "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
        matched_rule: "Preset: Antigravity",
      },
      {
        pid: 22410,
        parent_pid: 4120,
        name: "Code.exe",
        exe_path: "C:\\Users\\Administrator\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
        matched_rule: null,
      },
      {
        pid: 42276,
        parent_pid: 4120,
        name: "xray.exe",
        exe_path: "C:\\Tools\\v2rayN\\bin\\xray\\xray.exe",
        matched_rule: null,
      },
    ];
    const needle = filter?.trim().toLowerCase();
    return needle
      ? sample.filter(
          (p) =>
            p.name.toLowerCase().includes(needle) ||
            (p.exe_path ?? "").toLowerCase().includes(needle)
        )
      : sample;
  },

  async runDoctor(): Promise<DoctorReport> {
    if (isTauriRuntime()) {
      return invoke<DoctorReport>("run_doctor");
    }
    return {
      elevated: true,
      windivert_dll_loaded: true,
      windivert_dll_path: "runtime/windivert\\WinDivert.dll",
      windivert_driver_opened: true,
      windivert_version: "v2.2",
      windivert_error: null,
      proxies: Object.entries(previewConfig.proxies).map(([name, p]) => ({
        name,
        report: {
          address: p.address,
          kind: p.type,
          tcp_reachable: true,
          latency_ms: 1,
          udp_associate_supported: p.type === "socks5",
          error: null,
        },
        local_listener_pid: p.address.startsWith("127.0.0.1") ? 42276 : null,
      })),
    };
  },

  async probeProxy(proxy: ProxyConfig): Promise<NamedProxyProbe> {
    if (isTauriRuntime()) {
      return invoke<NamedProxyProbe>("probe_proxy_endpoint", { proxy });
    }
    return {
      name: "probe",
      report: {
        address: proxy.address,
        kind: proxy.type,
        tcp_reachable: true,
        latency_ms: 1,
        udp_associate_supported: proxy.type === "socks5",
        error: null,
      },
      local_listener_pid: proxy.address.startsWith("127.0.0.1") ? 42276 : null,
    };
  },

  async listPresets(): Promise<PresetInfo[]> {
    if (isTauriRuntime()) {
      return invoke<PresetInfo[]>("list_presets");
    }
    return previewPresets;
  },

  async getServiceStatus(): Promise<ServiceStateSummary> {
    if (isTauriRuntime()) {
      return invoke<ServiceStateSummary>("get_service_status");
    }
    return { installed: true, running: true, state: "running" };
  },

  async manageWindowsService(
    action: "install" | "start" | "stop" | "uninstall"
  ): Promise<ServiceStateSummary> {
    if (isTauriRuntime()) {
      return invoke<ServiceStateSummary>("manage_windows_service", { action });
    }
    return {
      installed: action !== "uninstall",
      running: action === "install" || action === "start",
      state: action === "uninstall" ? "not_installed" : action === "stop" ? "stopped" : "running",
    };
  },

  async subscribeEvents(
    onEvent: (evt: EngineEvent) => void,
    onRefresh: () => void
  ): Promise<UnlistenFn> {
    if (!isTauriRuntime()) {
      return () => {};
    }
    const u1 = await listen<EngineEvent>("bypath://event", (e) => {
      onEvent(e.payload);
    });
    const u2 = await listen("bypath://refresh", () => {
      onRefresh();
    });
    return () => {
      u1();
      u2();
    };
  },
};
