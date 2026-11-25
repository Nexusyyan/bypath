import { describe, expect, it } from "vitest";
import {
  aggregateConnections,
  buildPageNumbers,
  connectionsToCsv,
  domainPatternMatches,
  filterAndSortConnections,
  isDomainBlockedByConfig,
  isDomainDirectByConfig,
  wildcardSuggestion,
} from "./connections";
import { formatBytes, formatDuration, formatUptime } from "./format";
import type { Config, ConnectionRecord } from "./types";

const sampleConns: ConnectionRecord[] = [
  {
    id: 1,
    pid: 100,
    process_name: "Antigravity.exe",
    exe_path: "C:\\Antigravity\\Antigravity.exe",
    protocol: "tcp",
    src_addr: "192.168.1.5:50001",
    dst_ip: "142.250.1.1",
    dst_port: 443,
    sniffed_host: "aicode.googleapis.com",
    sniff_kind: "SNI",
    rule_name: "Preset: Antigravity",
    proxy_name: "local_socks",
    action: "proxy",
    state: "active",
    bytes_tx: 2000,
    bytes_rx: 8000,
    started_at_ms: 1_700_000_000_000,
    updated_at_ms: 1_700_000_001_000,
    ended_at_ms: null,
    error: null,
  },
  {
    id: 2,
    pid: 101,
    process_name: "language_server.exe",
    exe_path: "C:\\Antigravity\\language_server.exe",
    protocol: "tcp",
    src_addr: "192.168.1.5:50002",
    dst_ip: "142.250.1.2",
    dst_port: 443,
    sniffed_host: "play.googleapis.com",
    sniff_kind: "SNI",
    rule_name: "Blocked Domains",
    proxy_name: null,
    action: "block",
    state: "blocked",
    bytes_tx: 0,
    bytes_rx: 0,
    started_at_ms: 1_700_000_002_000,
    updated_at_ms: 1_700_000_002_000,
    ended_at_ms: 1_700_000_002_000,
    error: "Blocked domain",
  },
];

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
      address: "127.0.0.1:7897",
      udp: true,
    },
  },
  rules: [
    {
      name: "Direct Corp",
      enabled: true,
      process_names: [],
      path_prefixes: [],
      path_contains: [],
      include_children: true,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains: ["*.corp.internal"],
      action: "direct",
      proxy: null,
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

describe("domain matching and config helpers", () => {
  it("matches exact, suffix, and glob patterns", () => {
    expect(domainPatternMatches("play.googleapis.com", "PLAY.googleapis.com.")).toBe(true);
    expect(domainPatternMatches("*.doubleclick.net", "ad.doubleclick.net")).toBe(true);
    expect(domainPatternMatches("*.doubleclick.net", "notdoubleclick.net")).toBe(false);
    expect(domainPatternMatches("*telemetry*.example.com", "us-telemetry.example.com")).toBe(true);
  });

  it("generates wildcard suggestions cleanly", () => {
    expect(wildcardSuggestion("ad.doubleclick.net")).toBe("*.doubleclick.net");
    expect(wildcardSuggestion("example.com")).toBe("*.example.com");
    expect(wildcardSuggestion("localhost")).toBeNull();
  });

  it("detects blocked and direct domain coverage from config", () => {
    expect(isDomainBlockedByConfig(sampleConfig, "ad.doubleclick.net")).toBe(true);
    expect(isDomainBlockedByConfig(sampleConfig, "aicode.googleapis.com")).toBe(false);
    expect(isDomainDirectByConfig(sampleConfig, "git.corp.internal")).toBe(true);
    expect(isDomainDirectByConfig(sampleConfig, "github.com")).toBe(false);
  });
});

describe("connection filtering, sorting, aggregation, and CSV", () => {
  it("filters by state and sorts by transfer descending", () => {
    const activeOnly = filterAndSortConnections(
      sampleConns,
      "all",
      "",
      "active",
      "transfer",
      "desc"
    );
    expect(activeOnly).toHaveLength(1);
    expect(activeOnly[0].id).toBe(1);

    const byTimeDesc = filterAndSortConnections(
      sampleConns,
      "all",
      "",
      "all",
      "time",
      "desc"
    );
    expect(byTimeDesc.map((c) => c.id)).toEqual([2, 1]);
  });

  it("aggregates traffic by process and by domain", () => {
    const byProc = aggregateConnections(sampleConns, "process");
    expect(byProc[0].key).toBe("Antigravity.exe");
    expect(byProc[0].bytesTx + byProc[0].bytesRx).toBe(10_000);

    const byDom = aggregateConnections(sampleConns, "domain");
    expect(byDom[0].key).toBe("aicode.googleapis.com");
    expect(byDom[1].blocked).toBe(1);
  });

  it("builds compact pagination numbers and valid CSV", () => {
    expect(buildPageNumbers(1, 3)).toEqual([1, 2, 3]);
    expect(buildPageNumbers(5, 10)).toEqual([1, "ellipsis", 4, 5, 6, "ellipsis", 10]);
    const csv = connectionsToCsv(sampleConns);
    expect(csv.split("\n")).toHaveLength(3);
    expect(csv).toContain("aicode.googleapis.com");
  });
});

describe("format utilities", () => {
  it("formats bytes, uptime, and duration", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1536)).toBe("1.50 KB");
    expect(formatUptime(3661)).toBe("01:01:01");
    expect(formatDuration(1000, 1450)).toBe("450 ms");
    expect(formatDuration(1000, 4500)).toBe("3.5 s");
    expect(formatDuration(1000, 126_000)).toBe("2m 5s");
  });
});
