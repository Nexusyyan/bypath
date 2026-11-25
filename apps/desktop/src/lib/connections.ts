import type {
  Config,
  ConnectionRecord,
  ConnectionState,
  SortDir,
  SortKey,
  StateFilter,
} from "./types";

const STATE_RANK: Record<ConnectionState, number> = {
  active: 3,
  blocked: 2,
  failed: 1,
  closed: 0,
};

export function defaultSortDir(key: SortKey): SortDir {
  switch (key) {
    case "process":
    case "destination":
    case "proto":
      return "asc";
    default:
      return "desc";
  }
}

export function wildcardSuggestion(host: string): string | null {
  const clean = host.trim().replace(/\.+$/, "").toLowerCase();
  const labels = clean.split(".").filter(Boolean);
  if (labels.length >= 3) {
    return `*.${labels.slice(1).join(".")}`;
  }
  if (labels.length === 2) {
    return `*.${clean}`;
  }
  return null;
}

export function domainPatternMatches(patternRaw: string, hostRaw: string): boolean {
  const pat = patternRaw.trim().replace(/\.+$/, "").toLowerCase();
  const host = hostRaw.trim().replace(/\.+$/, "").toLowerCase();
  if (!pat || !host) return false;
  if (pat.startsWith("*.")) {
    const suffix = pat.slice(2);
    return host === suffix || host.endsWith(`.${suffix}`);
  }
  if (pat.startsWith(".")) {
    const suffix = pat.slice(1);
    return host === suffix || host.endsWith(`.${suffix}`);
  }
  if (pat.includes("*") || pat.includes("?")) {
    const escaped = pat
      .replace(/[.+^${}()|[\]\\]/g, "\\$&")
      .replace(/\*/g, ".*")
      .replace(/\?/g, ".");
    return new RegExp(`^${escaped}$`).test(host);
  }
  return host === pat;
}

export function isDomainBlockedByConfig(config: Config | null, host: string | null): boolean {
  if (!config || !host) return false;
  return config.rules.some(
    (r) =>
      r.enabled &&
      r.action === "block" &&
      (r.domains ?? []).some((pat) => domainPatternMatches(pat, host))
  );
}

export function isDomainDirectByConfig(config: Config | null, host: string | null): boolean {
  if (!config || !host) return false;
  return config.rules.some(
    (r) =>
      r.enabled &&
      r.action === "direct" &&
      (r.domains ?? []).some((pat) => domainPatternMatches(pat, host))
  );
}

export function buildPageNumbers(current: number, total: number): Array<number | "ellipsis"> {
  if (total <= 7) {
    return Array.from({ length: total }, (_, i) => i + 1);
  }
  const pages: Array<number | "ellipsis"> = [1];
  const left = Math.max(2, current - 1);
  const right = Math.min(total - 1, current + 1);
  if (left > 2) pages.push("ellipsis");
  for (let p = left; p <= right; p += 1) {
    pages.push(p);
  }
  if (right < total - 1) pages.push("ellipsis");
  pages.push(total);
  return pages;
}

export function filterAndSortConnections(
  connections: ConnectionRecord[],
  selectedProcess: string,
  query: string,
  filter: StateFilter,
  sortKey: SortKey,
  sortDir: SortDir
): ConnectionRecord[] {
  const q = query.trim().toLowerCase();
  const list = connections.filter((c) => {
    if (selectedProcess !== "all" && c.process_name !== selectedProcess) return false;
    if (filter === "active" && c.state !== "active") return false;
    if (filter === "blocked" && c.state !== "blocked" && c.state !== "failed") return false;
    if (!q) return true;
    return (
      c.process_name.toLowerCase().includes(q) ||
      (c.sniffed_host ?? "").toLowerCase().includes(q) ||
      c.dst_ip.toLowerCase().includes(q) ||
      String(c.dst_port).includes(q) ||
      (c.proxy_name ?? "").toLowerCase().includes(q) ||
      c.rule_name.toLowerCase().includes(q)
    );
  });

  const dir = sortDir === "asc" ? 1 : -1;
  list.sort((a, b) => {
    let cmp = 0;
    switch (sortKey) {
      case "time":
        cmp = a.started_at_ms - b.started_at_ms || a.id - b.id;
        break;
      case "transfer":
        cmp = a.bytes_tx + a.bytes_rx - (b.bytes_tx + b.bytes_rx);
        break;
      case "tx":
        cmp = a.bytes_tx - b.bytes_tx;
        break;
      case "rx":
        cmp = a.bytes_rx - b.bytes_rx;
        break;
      case "process":
        cmp = a.process_name.localeCompare(b.process_name);
        break;
      case "destination": {
        const da = a.sniffed_host ?? a.dst_ip;
        const db = b.sniffed_host ?? b.dst_ip;
        cmp = da.localeCompare(db) || a.dst_port - b.dst_port;
        break;
      }
      case "state":
        cmp = STATE_RANK[a.state] - STATE_RANK[b.state];
        break;
      case "proto":
        cmp = a.protocol.localeCompare(b.protocol);
        break;
    }
    if (cmp === 0) {
      return b.id - a.id;
    }
    return cmp * dir;
  });

  return list;
}

export interface TrafficSummaryRow {
  key: string;
  count: number;
  active: number;
  blocked: number;
  bytesTx: number;
  bytesRx: number;
  isDomain: boolean;
}

export function aggregateConnections(
  connections: ConnectionRecord[],
  groupBy: "process" | "domain"
): TrafficSummaryRow[] {
  const map = new Map<string, TrafficSummaryRow>();
  for (const c of connections) {
    const key =
      groupBy === "process" ? c.process_name : c.sniffed_host ?? `${c.dst_ip}:${c.dst_port}`;
    const isDomain = groupBy === "domain" && Boolean(c.sniffed_host);
    const cur = map.get(key);
    if (cur) {
      cur.count += 1;
      if (c.state === "active") cur.active += 1;
      if (c.state === "blocked" || c.state === "failed") cur.blocked += 1;
      cur.bytesTx += c.bytes_tx;
      cur.bytesRx += c.bytes_rx;
    } else {
      map.set(key, {
        key,
        count: 1,
        active: c.state === "active" ? 1 : 0,
        blocked: c.state === "blocked" || c.state === "failed" ? 1 : 0,
        bytesTx: c.bytes_tx,
        bytesRx: c.bytes_rx,
        isDomain,
      });
    }
  }

  return Array.from(map.values()).sort((a, b) => {
    const totalDiff = b.bytesTx + b.bytesRx - (a.bytesTx + a.bytesRx);
    if (totalDiff !== 0) return totalDiff;
    if (b.count !== a.count) return b.count - a.count;
    return a.key.localeCompare(b.key);
  });
}

function csvCell(value: string | number | null | undefined): string {
  const str = value == null ? "" : String(value);
  if (str.includes(",") || str.includes('"') || str.includes("\n")) {
    return `"${str.replace(/"/g, '""')}"`;
  }
  return str;
}

export function connectionsToCsv(connections: ConnectionRecord[]): string {
  const header = [
    "id",
    "started_at",
    "process_name",
    "pid",
    "protocol",
    "sniffed_host",
    "dst_ip",
    "dst_port",
    "action",
    "proxy_name",
    "rule_name",
    "state",
    "bytes_tx",
    "bytes_rx",
    "error",
  ];
  const lines = [header.join(",")];
  for (const c of connections) {
    lines.push(
      [
        c.id,
        new Date(c.started_at_ms).toISOString(),
        c.process_name,
        c.pid ?? "",
        c.protocol,
        c.sniffed_host ?? "",
        c.dst_ip,
        c.dst_port,
        c.action,
        c.proxy_name ?? "",
        c.rule_name,
        c.state,
        c.bytes_tx,
        c.bytes_rx,
        c.error ?? "",
      ]
        .map(csvCell)
        .join(",")
    );
  }
  return lines.join("\n");
}
