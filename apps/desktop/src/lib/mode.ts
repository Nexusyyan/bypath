import type { DaemonStatus, DoctorReport, ProxyRunMode } from "./types";

export type DriverReadiness =
  | "ready"
  | "files_ready_needs_privilege"
  | "blocked_by_security"
  | "missing"
  | "error"
  | "unknown";

export type RuntimeErrorCode =
  | "elevation_cancelled"
  | "service_cancelled"
  | "service_pipe_timeout"
  | "access_denied";

export function deriveRunMode(status: DaemonStatus | null): ProxyRunMode {
  if (!status || !status.engine_running) {
    return "off";
  }
  return status.backend_mode === "service" ? "background" : "foreground";
}

function classifyDriverFields(input: {
  driverReady: boolean;
  hasDll: boolean;
  error: string | null | undefined;
  errorKind: string | null | undefined;
  elevated: boolean;
  serviceRunning: boolean;
}): DriverReadiness {
  if (input.driverReady) {
    return "ready";
  }
  const kind = (input.errorKind ?? "").toLowerCase();
  const msg = (input.error ?? "").toLowerCase();

  if (
    kind === "blocked_by_security" ||
    kind === "invalid_signature" ||
    msg.includes("os error 1275") ||
    msg.includes("os error 577")
  ) {
    return "blocked_by_security";
  }

  if (
    kind === "missing_dll" ||
    kind === "missing_sys" ||
    msg.includes("windivert.dll not found") ||
    msg.includes("windivert64.sys")
  ) {
    return "missing";
  }

  const isAccessDenied =
    kind === "access_denied" ||
    msg.includes("os error 5") ||
    msg.includes("access denied");

  if (isAccessDenied && input.hasDll) {
    if (!input.elevated && !input.serviceRunning) {
      return "files_ready_needs_privilege";
    }
    return "error";
  }

  if (!input.hasDll) {
    return "missing";
  }

  return "error";
}

export function classifyDriverReadiness(status: DaemonStatus | null): DriverReadiness {
  if (!status) {
    return "unknown";
  }
  return classifyDriverFields({
    driverReady: status.windivert_ready,
    hasDll: Boolean(status.windivert_dll_path),
    error: status.windivert_error,
    errorKind: status.windivert_error_kind,
    elevated: status.elevated,
    serviceRunning: status.service_running,
  });
}

export function classifyDoctorDriverReadiness(
  doctor: DoctorReport | null,
  serviceRunning: boolean
): DriverReadiness {
  if (!doctor) {
    return "unknown";
  }
  return classifyDriverFields({
    driverReady: doctor.windivert_driver_opened,
    hasDll: doctor.windivert_dll_loaded || Boolean(doctor.windivert_dll_path),
    error: doctor.windivert_error,
    errorKind: doctor.windivert_error_kind,
    elevated: doctor.elevated,
    serviceRunning,
  });
}

export function needsDriverRecovery(status: DaemonStatus | null): boolean {
  const state = classifyDriverReadiness(status);
  return state === "blocked_by_security" || state === "missing" || state === "error";
}

export function classifyRuntimeError(rawError: string): RuntimeErrorCode | null {
  const trimmed = rawError.trim();
  if (trimmed === "ELEVATION_CANCELLED") {
    return "elevation_cancelled";
  }
  if (trimmed === "SERVICE_ACTION_CANCELLED") {
    return "service_cancelled";
  }
  if (trimmed === "SERVICE_PIPE_TIMEOUT") {
    return "service_pipe_timeout";
  }
  const lower = trimmed.toLowerCase();
  if (lower.includes("os error 5") || lower.includes("access denied")) {
    return "access_denied";
  }
  return null;
}
