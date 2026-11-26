import { deriveRunMode, needsDriverRecovery } from "./mode";
import type { Config, DaemonStatus, ProxyRunMode } from "./types";

export interface TraySummary {
  running: boolean;
  runMode: ProxyRunMode;
  proxyAddress: string | null;
  activeConnections: number;
  enabledRules: number;
  txBytesPerSec: number;
  rxBytesPerSec: number;
  needsAttention: boolean;
  isServiceMode: boolean;
}

export function extractFirstProxyAddress(config: Config | null): string | null {
  if (!config) return null;
  const first = Object.values(config.proxies)[0];
  return first?.address ?? null;
}

export function buildTraySummary(
  status: DaemonStatus | null,
  config: Config | null
): TraySummary {
  const runMode = deriveRunMode(status);
  const running = runMode !== "off";
  const proxyAddress = extractFirstProxyAddress(config);
  const enabledRules = config
    ? config.rules.filter((r) => r.enabled).length
    : status?.active_rules_count ?? 0;
  const needsAttention = needsDriverRecovery(status);
  return {
    running,
    runMode,
    proxyAddress,
    activeConnections: status?.stats.active_connections ?? 0,
    enabledRules,
    txBytesPerSec: status?.stats.tx_bytes_per_sec ?? 0,
    rxBytesPerSec: status?.stats.rx_bytes_per_sec ?? 0,
    needsAttention,
    isServiceMode: runMode === "background",
  };
}

export function buildTrayTooltip(
  running: boolean,
  runningLabel: string,
  standbyLabel: string
): string {
  return running ? `Bypath - ${runningLabel}` : `Bypath - ${standbyLabel}`;
}
