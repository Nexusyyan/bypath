import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AgentInfo,
  Config,
  ConfigSnapshot,
  ConnectionRecord,
  DaemonStatus,
  DoctorReport,
  EngineEvent,
  LogEntry,
  NamedProxyProbe,
  NavTab,
  PresetInfo,
  ProcessEntry,
  ProxyConfig,
  ProxyRunMode,
  RuleTestQuery,
  RuleTestReport,
  ServiceStateSummary,
  SetRunModeResult,
  StagedUpdateInfo,
  SystemProxyInfo,
  TrayStatePayload,
  UiPrefsSyncPayload,
  UpdateCheckReport,
  UpdateOutcomeRecord,
  UpdateProgressEvent,
  UpdateStateSnapshot,
} from "./types";
import { IDLE_UPDATE_PROGRESS } from "./update";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

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
    minimize_to_tray: true,
    auto_start_engine: false,
    theme: "system",
    auto_check_updates: true,
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

const previewScenario =
  typeof window !== "undefined"
    ? new URLSearchParams(window.location.search).get("preview")
    : null;

let previewRunMode: ProxyRunMode =
  previewScenario === "standard-stopped"
    ? "off"
    : previewScenario === "foreground"
      ? "foreground"
      : previewScenario === "off"
        ? "off"
        : "background";
let previewElevated = previewScenario !== "standard-stopped";
let previewServiceInstalled = previewScenario !== "fresh";
let previewServiceRunning =
  previewScenario !== "standard-stopped" && previewScenario !== "fresh";
let previewLaunchAtLogin = false;
let previewUpdateProgress: UpdateProgressEvent = { ...IDLE_UPDATE_PROGRESS };
let previewStagedUpdate: StagedUpdateInfo | null = null;
let previewUpdateCancelled = false;
const previewUpdateListeners = new Set<(evt: UpdateProgressEvent) => void>();

function emitPreviewUpdateProgress(evt: UpdateProgressEvent) {
  previewUpdateProgress = evt;
  for (const fn of previewUpdateListeners) {
    fn(evt);
  }
}
const now = Date.now();
let previewConnections: ConnectionRecord[] = [
  {
    id: 32,
    pid: 43180,
    process_name: "language_server.exe",
    exe_path:
      "C:\\Users\\User\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
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
      "C:\\Users\\User\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
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
      "C:\\Users\\User\\AppData\\Local\\Programs\\Antigravity\\resources\\bin\\language_server.exe",
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
];

const previewLogs: LogEntry[] = [
  {
    id: 2,
    timestamp_ms: now - 8_000,
    level: "INFO",
    target: "bypath_core::engine",
    message: "bypath engine started",
  },
  {
    id: 1,
    timestamp_ms: now - 12_000,
    level: "INFO",
    target: "bypath_windivert::windows_engine",
    message: "loaded WinDivert.dll dynamically",
  },
];

function buildPreviewStatus(): DaemonStatus {
  const running = previewRunMode !== "off";
  const active = previewConnections.filter((c) => c.state === "active").length;
  const blocked = previewConnections.filter((c) => c.state === "blocked").length;
  const backendMode =
    previewRunMode === "foreground"
      ? "embedded"
      : previewServiceRunning
        ? "service"
        : "embedded";
  const effectiveElevated = backendMode === "service" || previewElevated;
  const driverReady = effectiveElevated;
  return {
    backend_mode: backendMode,
    daemon_pid: backendMode === "service" ? 18420 : 24910,
    elevated: effectiveElevated,
    service_installed: previewServiceInstalled,
    service_running: previewServiceRunning,
    engine_running: running,
    windivert_ready: driverReady,
    windivert_dll_path: "runtime\\windivert\\WinDivert.dll",
    windivert_version: driverReady ? "v2.2" : null,
    windivert_error: driverReady
      ? null
      : "WinDivertOpen(layer=0, filter='false') failed with OS error 5 (Access Denied).",
    windivert_error_kind: driverReady ? null : "access_denied",
    portable_mode: false,
    config_path: "C:\\ProgramData\\Bypath\\config.toml",
    active_rules_count: previewConfig.rules.filter((r) => r.enabled).length,
    total_rules_count: previewConfig.rules.length,
    proxies_count: Object.keys(previewConfig.proxies).length,
    last_error: null,
    stats: {
      running,
      uptime_secs: running ? 842 : 0,
      active_connections: running ? active : 0,
      total_connections: previewConnections.length,
      proxied_connections: previewConnections.length - blocked,
      blocked_connections: blocked,
      direct_packets: 12_480,
      total_bytes_tx: 68_160,
      total_bytes_rx: 797_380,
      tx_bytes_per_sec: running ? 6_420 : 0,
      rx_bytes_per_sec: running ? 48_960 : 0,
    },
  };
}

export const api = {
  async getStatus(): Promise<DaemonStatus> {
    if (isTauriRuntime()) {
      return invoke<DaemonStatus>("get_status");
    }
    return buildPreviewStatus();
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
    previewRunMode = previewServiceRunning ? "background" : "foreground";
    previewElevated = true;
    return "engine started";
  },

  async stopEngine(): Promise<string> {
    if (isTauriRuntime()) {
      return invoke<string>("stop_engine");
    }
    previewRunMode = "off";
    return "engine stopped";
  },

  async setRunMode(mode: ProxyRunMode): Promise<SetRunModeResult> {
    if (isTauriRuntime()) {
      return invoke<SetRunModeResult>("set_run_mode", { mode });
    }
    if (mode === "off") {
      previewRunMode = "off";
    } else if (mode === "foreground") {
      previewElevated = true;
      previewRunMode = "foreground";
    } else {
      previewServiceInstalled = true;
      previewServiceRunning = true;
      previewRunMode = "background";
    }
    return {
      mode,
      elevating: false,
      status: buildPreviewStatus(),
    };
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
          "C:\\Users\\User\\AppData\\Local\\Programs\\Antigravity\\Antigravity.exe",
        matched_rule: "Preset: Antigravity",
      },
      {
        pid: 22410,
        parent_pid: 4120,
        name: "Code.exe",
        exe_path: "C:\\Users\\User\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
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
    const st = buildPreviewStatus();
    return {
      elevated: st.elevated,
      windivert_dll_loaded: true,
      windivert_dll_path: st.windivert_dll_path,
      windivert_driver_opened: st.windivert_ready,
      windivert_version: st.windivert_version,
      windivert_error: st.windivert_error,
      windivert_error_kind: st.windivert_error_kind,
      proxies: Object.entries(previewConfig.proxies).map(([name, p]) => ({
        name,
        report: {
          address: p.address,
          kind: p.type,
          tcp_reachable: true,
          handshake_ok: true,
          auth_ok: null,
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
        handshake_ok: true,
        auth_ok: null,
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

  async getSystemProxy(): Promise<SystemProxyInfo> {
    if (isTauriRuntime()) {
      return invoke<SystemProxyInfo>("get_system_proxy");
    }
    return {
      enabled: true,
      source: "windows_registry",
      raw_server: "127.0.0.1:7897",
      socks_address: null,
      http_address: "127.0.0.1:7897",
      bypass_list: "localhost;<local>",
      suggested_kind: "http",
      suggested_address: "127.0.0.1:7897",
    };
  },

  async testRule(query: RuleTestQuery): Promise<RuleTestReport> {
    if (isTauriRuntime()) {
      return invoke<RuleTestReport>("test_rule", { query });
    }
    return {
      phase1_decision: "proxy",
      phase1_rule: "Preset: Antigravity",
      phase1_reason: "matched proxy rule -> local_socks",
      phase2_decision: null,
      phase2_rule: null,
      final_action: "proxy",
      final_proxy: "local_socks",
      matched_rule: "Preset: Antigravity",
    };
  },

  async getLogs(minLevel?: string, limit = 150): Promise<LogEntry[]> {
    if (isTauriRuntime()) {
      return invoke<LogEntry[]>("get_logs", {
        minLevel: minLevel ?? null,
        limit,
      });
    }
    return previewLogs;
  },

  async exportConfigToml(): Promise<string> {
    if (isTauriRuntime()) {
      return invoke<string>("export_config_toml");
    }
    return JSON.stringify(previewConfig, null, 2);
  },

  async importConfigToml(rawToml: string): Promise<Config> {
    if (isTauriRuntime()) {
      return invoke<Config>("import_config_toml", { rawToml });
    }
    return structuredClone(previewConfig);
  },

  async getAgentInfo(): Promise<AgentInfo> {
    if (isTauriRuntime()) {
      return invoke<AgentInfo>("get_agent_info");
    }
    return {
      cli_path: "bypath.exe",
      pipe_name: "\\\\.\\pipe\\bypath-v1",
      config_path: "C:\\ProgramData\\Bypath\\config.toml",
      windivert_dir: "runtime\\windivert",
      portable_mode: false,
    };
  },

  async setTrayState(payload: TrayStatePayload): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("set_tray_state", { payload });
    }
  },

  async openMainWindow(tab?: NavTab): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("open_main_window", { tab: tab ?? null });
    }
  },

  async hideTrayMenu(): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("hide_tray_menu");
    }
  },

  async resizeTrayMenu(width: number, height: number): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("resize_tray_menu", { width, height });
    }
  },

  async syncUiPrefs(payload: UiPrefsSyncPayload): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("sync_ui_prefs", {
        locale: payload.locale ?? null,
        theme: payload.theme ?? null,
      });
    }
  },

  async quitApp(): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("quit_app");
    }
  },

  async openPathInExplorer(path: string): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("open_path_in_explorer", { path });
    }
  },

  async openWindowsSecurity(): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("open_windows_security");
    }
  },

  async checkForUpdates(): Promise<UpdateCheckReport> {
    if (isTauriRuntime()) {
      return invoke<UpdateCheckReport>("check_for_updates");
    }
    if (previewScenario === "update-available") {
      return {
        current_version: "0.4.3",
        latest_version: "0.4.4",
        latest_tag: "v0.4.4",
        update_available: true,
        release_name: "Bypath v0.4.4",
        release_url: "https://github.com/Nexusyyan/bypath/releases/tag/v0.4.4",
        published_at: new Date().toISOString(),
        release_notes:
          "## Highlights\n- In-app one-click update with SHA-256 verification\n- Seamless Restart to Update with automatic BypathService recovery",
        primary_asset_name: "bypath-v0.4.4-windows-x64.zip",
        primary_asset_url:
          "https://github.com/Nexusyyan/bypath/releases/download/v0.4.4/bypath-v0.4.4-windows-x64.zip",
        primary_asset_size: 8_420_000,
        nodriver_asset_name: "bypath-v0.4.4-windows-x64-nodriver.zip",
        nodriver_asset_url:
          "https://github.com/Nexusyyan/bypath/releases/download/v0.4.4/bypath-v0.4.4-windows-x64-nodriver.zip",
        nodriver_asset_size: 8_120_000,
        gui_exe_asset_name: "bypath-gui-v0.4.4-windows-x64.exe",
        gui_exe_asset_url:
          "https://github.com/Nexusyyan/bypath/releases/download/v0.4.4/bypath-gui-v0.4.4-windows-x64.exe",
        gui_exe_asset_size: 4_300_000,
        checksums_asset_url:
          "https://github.com/Nexusyyan/bypath/releases/download/v0.4.4/SHA256SUMS.txt",
        checked_via: "proxy:local_socks",
        checked_at_ms: Date.now(),
      };
    }
    return {
      current_version: "0.4.3",
      latest_version: "0.4.3",
      latest_tag: "v0.4.3",
      update_available: false,
      release_name: "Bypath v0.4.3",
      release_url: "https://github.com/Nexusyyan/bypath/releases",
      published_at: new Date().toISOString(),
      release_notes: null,
      primary_asset_name: null,
      primary_asset_url: null,
      primary_asset_size: null,
      nodriver_asset_name: null,
      nodriver_asset_url: null,
      nodriver_asset_size: null,
      gui_exe_asset_name: null,
      gui_exe_asset_url: null,
      gui_exe_asset_size: null,
      checksums_asset_url: null,
      checked_via: "preview",
      checked_at_ms: Date.now(),
    };
  },

  async getUpdateState(): Promise<UpdateStateSnapshot> {
    if (isTauriRuntime()) {
      return invoke<UpdateStateSnapshot>("get_update_state");
    }
    return {
      progress: { ...previewUpdateProgress },
      staged: previewStagedUpdate ? { ...previewStagedUpdate } : null,
    };
  },

  async downloadUpdate(report: UpdateCheckReport): Promise<StagedUpdateInfo> {
    if (isTauriRuntime()) {
      return invoke<StagedUpdateInfo>("download_update", { report });
    }
    previewUpdateCancelled = false;
    const total = report.nodriver_asset_size ?? report.primary_asset_size ?? 8_120_000;
    const assetName =
      report.nodriver_asset_name ?? report.primary_asset_name ?? "bypath-update.zip";
    for (const pct of [18, 46, 74, 100]) {
      await new Promise((r) => window.setTimeout(r, 180));
      if (previewUpdateCancelled) {
        emitPreviewUpdateProgress({ ...IDLE_UPDATE_PROGRESS });
        throw new Error("UPDATE_CANCELLED");
      }
      emitPreviewUpdateProgress({
        stage: "downloading",
        version: report.latest_version,
        tag: report.latest_tag,
        asset_name: assetName,
        downloaded_bytes: Math.round((total * pct) / 100),
        total_bytes: total,
        percent: pct,
        route: report.checked_via,
        error: null,
      });
    }
    emitPreviewUpdateProgress({
      stage: "verifying",
      version: report.latest_version,
      tag: report.latest_tag,
      asset_name: assetName,
      downloaded_bytes: total,
      total_bytes: total,
      percent: 100,
      route: report.checked_via,
      error: null,
    });
    await new Promise((r) => window.setTimeout(r, 220));
    emitPreviewUpdateProgress({
      stage: "extracting",
      version: report.latest_version,
      tag: report.latest_tag,
      asset_name: assetName,
      downloaded_bytes: total,
      total_bytes: total,
      percent: 100,
      route: report.checked_via,
      error: null,
    });
    await new Promise((r) => window.setTimeout(r, 220));
    const staged: StagedUpdateInfo = {
      target_version: report.latest_version,
      target_tag: report.latest_tag,
      asset_name: assetName,
      sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
      downloaded_via: report.checked_via,
      staged_at_ms: Date.now(),
    };
    previewStagedUpdate = staged;
    emitPreviewUpdateProgress({
      stage: "ready",
      version: report.latest_version,
      tag: report.latest_tag,
      asset_name: assetName,
      downloaded_bytes: total,
      total_bytes: total,
      percent: 100,
      route: report.checked_via,
      error: null,
    });
    return staged;
  },

  async cancelUpdateDownload(): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("cancel_update_download");
      return;
    }
    previewUpdateCancelled = true;
  },

  async installUpdateAndRestart(): Promise<boolean> {
    if (isTauriRuntime()) {
      return invoke<boolean>("install_update_and_restart");
    }
    if (!previewStagedUpdate) {
      throw new Error("No downloaded update is ready to install");
    }
    emitPreviewUpdateProgress({
      stage: "applying",
      version: previewStagedUpdate.target_version,
      tag: previewStagedUpdate.target_tag,
      asset_name: previewStagedUpdate.asset_name,
      downloaded_bytes: 0,
      total_bytes: null,
      percent: 100,
      route: previewStagedUpdate.downloaded_via,
      error: null,
    });
    return true;
  },

  async takeUpdateOutcome(): Promise<UpdateOutcomeRecord | null> {
    if (isTauriRuntime()) {
      return invoke<UpdateOutcomeRecord | null>("take_update_outcome");
    }
    return null;
  },

  async subscribeUpdateProgress(
    onProgress: (evt: UpdateProgressEvent) => void
  ): Promise<UnlistenFn> {
    if (isTauriRuntime()) {
      return listen<UpdateProgressEvent>("bypath://update-progress", (e) => {
        onProgress(e.payload);
      });
    }
    previewUpdateListeners.add(onProgress);
    return () => {
      previewUpdateListeners.delete(onProgress);
    };
  },

  async openExternalUrl(url: string): Promise<void> {
    if (isTauriRuntime()) {
      await invoke("open_external_url", { url });
      return;
    }
    window.open(url, "_blank", "noopener,noreferrer");
  },

  async pickDirectory(title?: string): Promise<string | null> {
    if (isTauriRuntime()) {
      return invoke<string | null>("pick_directory", { title: title ?? null });
    }
    return null;
  },

  async getLaunchAtLogin(): Promise<boolean> {
    if (isTauriRuntime()) {
      return invoke<boolean>("get_launch_at_login");
    }
    return previewLaunchAtLogin;
  },

  async setLaunchAtLogin(enabled: boolean): Promise<boolean> {
    if (isTauriRuntime()) {
      return invoke<boolean>("set_launch_at_login", { enabled });
    }
    previewLaunchAtLogin = enabled;
    return previewLaunchAtLogin;
  },

  async getServiceStatus(): Promise<ServiceStateSummary> {
    if (isTauriRuntime()) {
      return invoke<ServiceStateSummary>("get_service_status");
    }
    return {
      installed: previewServiceInstalled,
      running: previewServiceRunning,
      state: !previewServiceInstalled
        ? "not_installed"
        : previewServiceRunning
          ? "running"
          : "stopped",
    };
  },

  async manageWindowsService(
    action: "install" | "start" | "stop" | "uninstall"
  ): Promise<ServiceStateSummary> {
    if (isTauriRuntime()) {
      return invoke<ServiceStateSummary>("manage_windows_service", { action });
    }
    previewServiceInstalled = action !== "uninstall";
    previewServiceRunning = action === "install" || action === "start";
    if (!previewServiceRunning && previewRunMode === "background") {
      previewRunMode = "off";
    }
    return {
      installed: previewServiceInstalled,
      running: previewServiceRunning,
      state: !previewServiceInstalled
        ? "not_installed"
        : previewServiceRunning
          ? "running"
          : "stopped",
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

  async subscribeNavigate(onNavigate: (tab: NavTab) => void): Promise<UnlistenFn> {
    if (!isTauriRuntime()) {
      return () => {};
    }
    return listen<string>("bypath://navigate", (e) => {
      const tab = e.payload;
      if (
        tab === "connections" ||
        tab === "applications" ||
        tab === "proxies" ||
        tab === "diagnostics"
      ) {
        onNavigate(tab);
      }
    });
  },

  async subscribeTrayOpened(onOpened: () => void): Promise<UnlistenFn> {
    if (!isTauriRuntime()) {
      return () => {};
    }
    return listen("bypath://tray-opened", () => {
      onOpened();
    });
  },

  async subscribeUiPrefs(
    onPrefs: (payload: UiPrefsSyncPayload) => void
  ): Promise<UnlistenFn> {
    if (!isTauriRuntime()) {
      return () => {};
    }
    return listen<UiPrefsSyncPayload>("bypath://ui-prefs", (e) => {
      onPrefs(e.payload);
    });
  },
};
