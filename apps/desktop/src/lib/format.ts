export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let val = bytes;
  let idx = 0;
  while (val >= 1024 && idx < units.length - 1) {
    val /= 1024;
    idx += 1;
  }
  const decimals = idx === 0 ? 0 : val >= 100 ? 0 : val >= 10 ? 1 : 2;
  return `${val.toFixed(decimals)} ${units[idx]}`;
}

export function formatSpeed(bytesPerSec: number): string {
  return `${formatBytes(bytesPerSec)}/s`;
}

export function formatUptime(secs: number): string {
  if (!secs || secs < 0) return "00:00:00";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  return [h, m, s].map((v) => String(v).padStart(2, "0")).join(":");
}

export function formatTime(tsMs: number): string {
  if (!tsMs) return "--:--:--";
  const d = new Date(tsMs);
  return d.toLocaleTimeString("en-GB", { hour12: false });
}

export function formatDuration(startedAtMs: number, endedAtMs: number | null): string {
  if (!startedAtMs) return "-";
  const end = endedAtMs ?? Date.now();
  const diffMs = Math.max(0, end - startedAtMs);
  if (diffMs < 1000) return `${diffMs} ms`;
  const secs = diffMs / 1000;
  if (secs < 60) return `${secs.toFixed(1)} s`;
  const mins = Math.floor(secs / 60);
  const rem = Math.floor(secs % 60);
  return `${mins}m ${rem}s`;
}
