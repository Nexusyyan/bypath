import { describe, expect, it } from "vitest";
import {
  classifyDoctorDriverReadiness,
  classifyDriverReadiness,
  classifyRuntimeError,
  deriveRunMode,
  needsDriverRecovery,
} from "./mode";
import type { DaemonStatus, DoctorReport } from "./types";

const baseStatus: DaemonStatus = {
  backend_mode: "service",
  daemon_pid: 1000,
  elevated: true,
  service_installed: true,
  service_running: true,
  engine_running: true,
  windivert_ready: true,
  windivert_dll_path: "runtime\\windivert\\WinDivert.dll",
  windivert_version: "v2.2",
  windivert_error: null,
  windivert_error_kind: null,
  config_path: "C:\\ProgramData\\Bypath\\config.toml",
  active_rules_count: 1,
  total_rules_count: 1,
  proxies_count: 1,
  last_error: null,
  stats: {
    running: true,
    uptime_secs: 60,
    active_connections: 2,
    total_connections: 10,
    proxied_connections: 10,
    blocked_connections: 0,
    direct_packets: 100,
    total_bytes_tx: 1024,
    total_bytes_rx: 2048,
    tx_bytes_per_sec: 100,
    rx_bytes_per_sec: 200,
  },
};

describe("proxy run mode derivation", () => {
  it("distinguishes off, foreground, and background states cleanly", () => {
    expect(deriveRunMode(null)).toBe("off");
    expect(deriveRunMode({ ...baseStatus, engine_running: false })).toBe("off");
    expect(deriveRunMode(baseStatus)).toBe("background");
    expect(
      deriveRunMode({
        ...baseStatus,
        backend_mode: "embedded",
        service_running: false,
        engine_running: true,
      })
    ).toBe("foreground");
  });
});

describe("driver readiness classification", () => {
  it("treats non-elevated OS error 5 with present DLL as ready-for-privilege, not a broken driver", () => {
    const standardStopped: DaemonStatus = {
      ...baseStatus,
      backend_mode: "embedded",
      elevated: false,
      service_installed: true,
      service_running: false,
      engine_running: false,
      windivert_ready: false,
      windivert_dll_path: "C:\\Bypath\\runtime\\windivert\\WinDivert.dll",
      windivert_version: null,
      windivert_error:
        "WinDivertOpen(layer=0, filter='false') failed with OS error 5 (Access Denied).",
      windivert_error_kind: "access_denied",
    };

    expect(classifyDriverReadiness(standardStopped)).toBe("files_ready_needs_privilege");
    expect(needsDriverRecovery(standardStopped)).toBe(false);
  });

  it("flags genuine missing or security-blocked drivers for recovery", () => {
    const missing: DaemonStatus = {
      ...baseStatus,
      windivert_ready: false,
      windivert_dll_path: null,
      windivert_error_kind: "missing_dll",
      windivert_error: "WinDivert.dll not found",
    };
    expect(classifyDriverReadiness(missing)).toBe("missing");
    expect(needsDriverRecovery(missing)).toBe(true);

    const blocked: DaemonStatus = {
      ...baseStatus,
      windivert_ready: false,
      windivert_error_kind: "blocked_by_security",
      windivert_error: "OS error 1275",
    };
    expect(classifyDriverReadiness(blocked)).toBe("blocked_by_security");
    expect(needsDriverRecovery(blocked)).toBe(true);
  });

  it("classifies doctor report driver state consistently with status", () => {
    const standardDoctor: DoctorReport = {
      elevated: false,
      windivert_dll_loaded: true,
      windivert_dll_path: "runtime\\windivert\\WinDivert.dll",
      windivert_driver_opened: false,
      windivert_version: null,
      windivert_error: "WinDivertOpen failed with OS error 5 (Access Denied).",
      windivert_error_kind: "access_denied",
      proxies: [],
    };
    expect(classifyDoctorDriverReadiness(standardDoctor, false)).toBe(
      "files_ready_needs_privilege"
    );
    expect(classifyDoctorDriverReadiness(standardDoctor, true)).toBe("error");
  });

  it("maps structured backend error codes for user-friendly localization", () => {
    expect(classifyRuntimeError("ELEVATION_CANCELLED")).toBe("elevation_cancelled");
    expect(classifyRuntimeError("SERVICE_ACTION_CANCELLED")).toBe("service_cancelled");
    expect(classifyRuntimeError("SERVICE_PIPE_TIMEOUT")).toBe("service_pipe_timeout");
    expect(
      classifyRuntimeError("WinDivertOpen failed with OS error 5 (Access Denied)")
    ).toBe("access_denied");
    expect(classifyRuntimeError("upstream dial timed out")).toBeNull();
  });
});
