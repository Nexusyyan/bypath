import { describe, expect, it } from "vitest";
import enMessages from "../locales/en.json";
import zhCnMessages from "../locales/zh-CN.json";
import { buildTraySummary, buildTrayTooltip, extractFirstProxyAddress } from "./tray";
import type { Config, DaemonStatus } from "./types";

const sampleConfig: Config = {
  general: {
    log_level: "info",
    enable_sniffing: true,
    block_quic_when_no_udp: true,
    bypass_lan: true,
    windivert_dir: "runtime/windivert",
    minimize_to_tray: true,
    auto_start_engine: false,
    theme: "system",
  },
  proxies: {
    local_socks: {
      type: "socks5",
      address: "127.0.0.1:10808",
      udp: true,
    },
  },
  rules: [
    {
      name: "Preset: Antigravity",
      enabled: true,
      process_names: ["Antigravity.exe"],
      path_prefixes: [],
      path_contains: [],
      include_children: true,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains: [],
      action: "proxy",
      proxy: "local_socks",
    },
    {
      name: "Disabled Rule",
      enabled: false,
      process_names: ["curl.exe"],
      path_prefixes: [],
      path_contains: [],
      include_children: true,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains: [],
      action: "proxy",
      proxy: "local_socks",
    },
  ],
};

const sampleStatus: DaemonStatus = {
  backend_mode: "service",
  daemon_pid: 4200,
  elevated: true,
  service_installed: true,
  service_running: true,
  engine_running: true,
  windivert_ready: true,
  windivert_dll_path: "runtime\\windivert\\WinDivert.dll",
  windivert_version: "v2.2",
  windivert_error: null,
  config_path: "C:\\ProgramData\\Bypath\\config.toml",
  active_rules_count: 1,
  total_rules_count: 2,
  proxies_count: 1,
  last_error: null,
  stats: {
    running: true,
    uptime_secs: 120,
    active_connections: 4,
    total_connections: 18,
    proxied_connections: 16,
    blocked_connections: 2,
    direct_packets: 240,
    total_bytes_tx: 4096,
    total_bytes_rx: 65536,
    tx_bytes_per_sec: 1024,
    rx_bytes_per_sec: 8192,
  },
};

function extractPlaceholders(template: string): string[] {
  return Array.from(template.matchAll(/\{(\w+)\}/g), (m) => m[1]).sort();
}

describe("tray summary and tooltip helpers", () => {
  it("builds accurate tray summary from daemon status and config", () => {
    expect(extractFirstProxyAddress(sampleConfig)).toBe("127.0.0.1:10808");
    expect(extractFirstProxyAddress(null)).toBeNull();

    const summary = buildTraySummary(sampleStatus, sampleConfig);
    expect(summary.running).toBe(true);
    expect(summary.proxyAddress).toBe("127.0.0.1:10808");
    expect(summary.activeConnections).toBe(4);
    expect(summary.enabledRules).toBe(1);
    expect(summary.txBytesPerSec).toBe(1024);
    expect(summary.rxBytesPerSec).toBe(8192);
    expect(summary.needsAttention).toBe(false);
    expect(summary.isServiceMode).toBe(true);
  });

  it("flags attention when driver or privilege readiness is missing", () => {
    const degraded: DaemonStatus = {
      ...sampleStatus,
      engine_running: false,
      windivert_ready: false,
      service_running: false,
      elevated: false,
    };
    const summary = buildTraySummary(degraded, sampleConfig);
    expect(summary.running).toBe(false);
    expect(summary.needsAttention).toBe(true);
  });

  it("formats localized tray tooltip cleanly", () => {
    expect(buildTrayTooltip(true, "Running - 127.0.0.1:10808", "Standby")).toBe(
      "Bypath - Running - 127.0.0.1:10808"
    );
    expect(buildTrayTooltip(false, "Running - 127.0.0.1:10808", "Standby")).toBe(
      "Bypath - Standby"
    );
  });
});

describe("bilingual locale dictionary parity", () => {
  it("keeps en.json and zh-CN.json keys and interpolation placeholders in exact parity", () => {
    const enKeys = Object.keys(enMessages).sort();
    const zhKeys = Object.keys(zhCnMessages).sort();
    expect(zhKeys).toEqual(enKeys);

    const trayKeys = enKeys.filter((k) => k.startsWith("tray."));
    expect(trayKeys.length).toBeGreaterThanOrEqual(10);

    for (const key of enKeys as Array<keyof typeof enMessages>) {
      expect(enMessages[key].trim().length).toBeGreaterThan(0);
      expect(zhCnMessages[key].trim().length).toBeGreaterThan(0);
      expect(extractPlaceholders(zhCnMessages[key])).toEqual(
        extractPlaceholders(enMessages[key])
      );
    }
  });
});
