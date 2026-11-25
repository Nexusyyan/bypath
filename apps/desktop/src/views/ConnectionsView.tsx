import { Fragment, useEffect, useMemo, useState } from "react";
import {
  ActivityIcon,
  BanIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  PauseIcon,
  PlayIcon,
  SearchIcon,
  SortAscIcon,
  SortDescIcon,
  SortNeutralIcon,
  TrashIcon,
} from "../components/Icons";
import { formatBytes, formatSpeed, formatTime, formatUptime } from "../lib/format";
import { useI18n } from "../lib/i18n";
import type { Config, ConnectionRecord, ConnectionState, DaemonStatus } from "../lib/types";

interface ConnectionsViewProps {
  status: DaemonStatus | null;
  config: Config | null;
  connections: ConnectionRecord[];
  paused: boolean;
  saving: boolean;
  onTogglePause: () => void;
  onClearClosed: () => void;
  onStartEngine: () => void;
  onOpenDiagnostics: () => void;
  onBlockDomain: (pattern: string) => void;
}

type StateFilter = "all" | "active" | "blocked";
type SortKey =
  | "time"
  | "transfer"
  | "tx"
  | "rx"
  | "process"
  | "destination"
  | "state"
  | "proto";
type SortDir = "asc" | "desc";

const PAGE_SIZES = [25, 50, 100] as const;

const STATE_RANK: Record<ConnectionState, number> = {
  active: 3,
  blocked: 2,
  failed: 1,
  closed: 0,
};

function defaultSortDir(key: SortKey): SortDir {
  switch (key) {
    case "process":
    case "destination":
    case "proto":
      return "asc";
    default:
      return "desc";
  }
}

function wildcardSuggestion(host: string): string | null {
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

function domainPatternMatches(patternRaw: string, hostRaw: string): boolean {
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
  if (pat.includes("*")) {
    const escaped = pat
      .replace(/[.+?^${}()|[\]\\]/g, "\\$&")
      .replace(/\*/g, ".*");
    return new RegExp(`^${escaped}$`).test(host);
  }
  return host === pat;
}

function isDomainBlockedByConfig(config: Config | null, host: string | null): boolean {
  if (!config || !host) return false;
  return config.rules.some(
    (r) =>
      r.enabled &&
      r.action === "block" &&
      (r.domains ?? []).some((pat) => domainPatternMatches(pat, host))
  );
}

function buildPageNumbers(current: number, total: number): Array<number | "ellipsis"> {
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

export function ConnectionsView({
  status,
  config,
  connections,
  paused,
  saving,
  onTogglePause,
  onClearClosed,
  onStartEngine,
  onOpenDiagnostics,
  onBlockDomain,
}: ConnectionsViewProps) {
  const { t } = useI18n();
  const [selectedProcess, setSelectedProcess] = useState<string>("all");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<StateFilter>("all");
  const [sortKey, setSortKey] = useState<SortKey>("time");
  const [sortDir, setSortDir] = useState<SortDir>("desc");
  const [pageSize, setPageSize] = useState<number>(25);
  const [page, setPage] = useState<number>(1);
  const [expandedId, setExpandedId] = useState<number | null>(null);

  const processTabs = useMemo(() => {
    const map = new Map<string, { name: string; total: number; active: number }>();
    for (const c of connections) {
      const existing = map.get(c.process_name);
      if (existing) {
        existing.total += 1;
        if (c.state === "active") existing.active += 1;
      } else {
        map.set(c.process_name, {
          name: c.process_name,
          total: 1,
          active: c.state === "active" ? 1 : 0,
        });
      }
    }
    return Array.from(map.values()).sort((a, b) => {
      if (b.active !== a.active) return b.active - a.active;
      if (b.total !== a.total) return b.total - a.total;
      return a.name.localeCompare(b.name);
    });
  }, [connections]);

  useEffect(() => {
    if (
      selectedProcess !== "all" &&
      !processTabs.some((p) => p.name === selectedProcess)
    ) {
      setSelectedProcess("all");
    }
  }, [processTabs, selectedProcess]);

  useEffect(() => {
    setPage(1);
  }, [selectedProcess, query, filter, sortKey, sortDir, pageSize]);

  const filteredAndSorted = useMemo(() => {
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
  }, [connections, selectedProcess, query, filter, sortKey, sortDir]);

  const totalCount = filteredAndSorted.length;
  const totalPages = Math.max(1, Math.ceil(totalCount / pageSize));
  const safePage = Math.min(page, totalPages);
  const pageStartIdx = (safePage - 1) * pageSize;
  const pagedRows = useMemo(
    () => filteredAndSorted.slice(pageStartIdx, pageStartIdx + pageSize),
    [filteredAndSorted, pageStartIdx, pageSize]
  );

  const handleHeaderSort = (key: SortKey) => {
    if (sortKey === key) {
      setSortDir((prev) => (prev === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir(defaultSortDir(key));
    }
  };

  const renderSortIcon = (key: SortKey) => {
    if (sortKey !== key) {
      return <SortNeutralIcon size={12} className="sort-icon-muted" />;
    }
    return sortDir === "asc" ? <SortAscIcon size={12} /> : <SortDescIcon size={12} />;
  };

  const running = status?.engine_running ?? false;
  const stats = status?.stats;
  const dllLoaded = Boolean(status?.windivert_dll_path);
  const accessDenied =
    (status?.windivert_error ?? "").toLowerCase().includes("access denied") ||
    (status?.windivert_error ?? "").includes("os error 5");

  const standbySubText = running
    ? t("connections.uptime", { uptime: formatUptime(stats?.uptime_secs ?? 0) })
    : status?.windivert_ready
      ? t("connections.windivertReady", {
          version: status.windivert_version ?? "v2.2",
        })
      : accessDenied
        ? t("connections.driverAccessDenied")
        : dllLoaded
          ? t("connections.driverCheckFailed")
          : t("connections.windivertMissing");

  const standbyEmptyDesc = running
    ? t("connections.emptyRunningDesc")
    : status?.windivert_ready
      ? t("connections.emptyReadyDesc")
      : accessDenied
        ? t("connections.emptyDeniedDesc")
        : dllLoaded
          ? status?.windivert_error ?? t("connections.emptyCheckDesc")
          : t("connections.emptyMissingDesc");

  const totalActiveCount = useMemo(
    () => connections.filter((c) => c.state === "active").length,
    [connections]
  );

  return (
    <div className="view-container">
      <div className="telemetry-strip" role="region" aria-label={t("connections.metricsAria")}>
        <div className="telemetry-cell">
          <span className="telemetry-label">{t("connections.interception")}</span>
          <span className="telemetry-value">
            <span className={`dot ${running ? "dot-ok" : "dot-idle"}`} />
            {running ? t("engine.running") : t("engine.standby")}
          </span>
          <span className="telemetry-sub">{standbySubText}</span>
        </div>

        <div className="telemetry-cell">
          <span className="telemetry-label">{t("connections.activeStreams")}</span>
          <span className="telemetry-value">{stats?.active_connections ?? 0}</span>
          <span className="telemetry-sub">
            {t("connections.streamSummary", {
              total: stats?.total_connections ?? 0,
              blocked: stats?.blocked_connections ?? 0,
            })}
          </span>
        </div>

        <div className="telemetry-cell">
          <span className="telemetry-label">{t("connections.upload")}</span>
          <span className="telemetry-value">{formatSpeed(stats?.tx_bytes_per_sec ?? 0)}</span>
          <span className="telemetry-sub">
            {t("connections.total", { size: formatBytes(stats?.total_bytes_tx ?? 0) })}
          </span>
        </div>

        <div className="telemetry-cell">
          <span className="telemetry-label">{t("connections.download")}</span>
          <span className="telemetry-value">{formatSpeed(stats?.rx_bytes_per_sec ?? 0)}</span>
          <span className="telemetry-sub">
            {t("connections.total", { size: formatBytes(stats?.total_bytes_rx ?? 0) })}
          </span>
        </div>
      </div>

      {processTabs.length > 0 && (
        <div
          className="process-tabs"
          role="tablist"
          aria-label={t("connections.processTabsAria")}
        >
          <button
            type="button"
            role="tab"
            aria-selected={selectedProcess === "all"}
            className={`process-tab ${selectedProcess === "all" ? "active" : ""}`}
            onClick={() => setSelectedProcess("all")}
          >
            {totalActiveCount > 0 && <span className="dot dot-ok" aria-hidden="true" />}
            <span>{t("connections.allProcesses")}</span>
            <span className="process-tab-count">{connections.length}</span>
          </button>

          {processTabs.map((proc) => (
            <button
              key={proc.name}
              type="button"
              role="tab"
              aria-selected={selectedProcess === proc.name}
              className={`process-tab ${selectedProcess === proc.name ? "active" : ""}`}
              onClick={() => setSelectedProcess(proc.name)}
              title={proc.name}
            >
              <span
                className={`dot ${proc.active > 0 ? "dot-ok" : "dot-idle"}`}
                aria-hidden="true"
              />
              <span className="process-tab-name">{proc.name}</span>
              <span className="process-tab-count">{proc.total}</span>
            </button>
          ))}
        </div>
      )}

      <div className="toolbar">
        <div className="search-box">
          <span className="search-icon">
            <SearchIcon size={14} />
          </span>
          <input
            className="input"
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("connections.searchPlaceholder")}
            aria-label={t("connections.searchAria")}
          />
        </div>

        <div className="toolbar-group">
          <div className="segmented" role="tablist" aria-label={t("connections.filterAria")}>
            {(["all", "active", "blocked"] as StateFilter[]).map((key) => (
              <button
                key={key}
                type="button"
                role="tab"
                aria-selected={filter === key}
                className={`segmented-btn ${filter === key ? "active" : ""}`}
                onClick={() => setFilter(key)}
              >
                {key === "all"
                  ? t("connections.filterAll")
                  : key === "active"
                    ? t("connections.filterActive")
                    : t("connections.filterBlocked")}
              </button>
            ))}
          </div>

          <div className="sort-control">
            <select
              className="select sort-select"
              value={sortKey}
              aria-label={t("connections.sortAria")}
              onChange={(e) => {
                const next = e.target.value as SortKey;
                setSortKey(next);
                setSortDir(defaultSortDir(next));
              }}
            >
              <option value="time">{t("connections.sortTime")}</option>
              <option value="transfer">{t("connections.sortTransfer")}</option>
              <option value="tx">{t("connections.sortTx")}</option>
              <option value="rx">{t("connections.sortRx")}</option>
              <option value="process">{t("connections.sortProcess")}</option>
              <option value="destination">{t("connections.sortDestination")}</option>
              <option value="state">{t("connections.sortState")}</option>
              <option value="proto">{t("connections.sortProto")}</option>
            </select>
            <button
              type="button"
              className="btn btn-sm btn-outline sort-dir-btn"
              aria-label={t("connections.sortDirectionAria")}
              title={t("connections.sortDirectionAria")}
              onClick={() => setSortDir((d) => (d === "asc" ? "desc" : "asc"))}
            >
              {sortDir === "asc" ? <SortAscIcon size={13} /> : <SortDescIcon size={13} />}
            </button>
          </div>

          <button
            type="button"
            className={`btn btn-sm ${paused ? "btn-secondary" : "btn-outline"}`}
            onClick={onTogglePause}
          >
            {paused ? <PlayIcon size={11} /> : <PauseIcon size={12} />}
            <span>{paused ? t("connections.resume") : t("connections.pause")}</span>
          </button>

          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={onClearClosed}
            disabled={connections.length === 0}
          >
            <TrashIcon size={13} />
            <span>{t("connections.clear")}</span>
          </button>
        </div>
      </div>

      <div className="table-wrap">
        {totalCount === 0 ? (
          <div className="empty-state">
            <div className="empty-icon">
              <ActivityIcon size={18} />
            </div>
            <h3 className="empty-title">
              {connections.length === 0
                ? running
                  ? t("connections.emptyRunningTitle")
                  : t("connections.emptyStandbyTitle")
                : t("connections.emptyFilteredTitle")}
            </h3>
            <p className="empty-desc">
              {connections.length === 0
                ? standbyEmptyDesc
                : t("connections.emptyFilteredDesc")}
            </p>
            {connections.length === 0 && !running && (
              <div className="toolbar-group" style={{ marginTop: 6 }}>
                {status?.windivert_ready ? (
                  <button type="button" className="btn btn-secondary" onClick={onStartEngine}>
                    {t("engine.start")}
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn btn-secondary"
                    onClick={onOpenDiagnostics}
                  >
                    {t("connections.openDiagnostics")}
                  </button>
                )}
              </div>
            )}
          </div>
        ) : (
          <>
            <table className="data-table">
              <thead>
                <tr>
                  <th style={{ width: "20%" }}>
                    <button
                      type="button"
                      className={`th-sort-btn ${sortKey === "process" ? "active" : ""}`}
                      onClick={() => handleHeaderSort("process")}
                    >
                      <span>{t("connections.colProcess")}</span>
                      {renderSortIcon("process")}
                    </button>
                  </th>
                  <th style={{ width: "35%" }}>
                    <button
                      type="button"
                      className={`th-sort-btn ${sortKey === "destination" ? "active" : ""}`}
                      onClick={() => handleHeaderSort("destination")}
                    >
                      <span>{t("connections.colDestination")}</span>
                      {renderSortIcon("destination")}
                    </button>
                  </th>
                  <th style={{ width: "8%" }}>
                    <button
                      type="button"
                      className={`th-sort-btn ${sortKey === "proto" ? "active" : ""}`}
                      onClick={() => handleHeaderSort("proto")}
                    >
                      <span>{t("connections.colProto")}</span>
                      {renderSortIcon("proto")}
                    </button>
                  </th>
                  <th style={{ width: "13%" }}>{t("connections.colUpstream")}</th>
                  <th style={{ width: "13%" }}>
                    <button
                      type="button"
                      className={`th-sort-btn ${
                        sortKey === "transfer" || sortKey === "tx" || sortKey === "rx"
                          ? "active"
                          : ""
                      }`}
                      onClick={() => handleHeaderSort("transfer")}
                    >
                      <span>{t("connections.colTransfer")}</span>
                      {renderSortIcon("transfer")}
                    </button>
                  </th>
                  <th style={{ width: "11%" }}>
                    <button
                      type="button"
                      className={`th-sort-btn ${sortKey === "state" ? "active" : ""}`}
                      onClick={() => handleHeaderSort("state")}
                    >
                      <span>{t("connections.colState")}</span>
                      {renderSortIcon("state")}
                    </button>
                  </th>
                </tr>
              </thead>
              <tbody>
                {pagedRows.map((conn) => {
                  const isExpanded = expandedId === conn.id;
                  const dotClass =
                    conn.state === "active"
                      ? "dot-ok"
                      : conn.state === "failed" || conn.state === "blocked"
                        ? "dot-err"
                        : "dot-idle";
                  const hostBlocked = isDomainBlockedByConfig(config, conn.sniffed_host);
                  const wildcardPat = conn.sniffed_host
                    ? wildcardSuggestion(conn.sniffed_host)
                    : null;

                  return (
                    <Fragment key={conn.id}>
                      <tr
                        className={`clickable ${isExpanded ? "selected-row" : ""}`}
                        onClick={() => setExpandedId(isExpanded ? null : conn.id)}
                      >
                        <td>
                          <div className="cell-primary" title={conn.process_name}>
                            {conn.process_name}
                          </div>
                          <div className="cell-secondary mono">
                            {conn.pid ? `PID ${conn.pid}` : t("connections.unknownPid")} ·{" "}
                            {formatTime(conn.started_at_ms)}
                          </div>
                        </td>
                        <td>
                          <div
                            className="cell-primary mono"
                            style={{ display: "flex", alignItems: "center", gap: 6 }}
                            title={
                              conn.sniffed_host
                                ? `${conn.sniffed_host}:${conn.dst_port}`
                                : `${conn.dst_ip}:${conn.dst_port}`
                            }
                          >
                            <span
                              style={{
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                              }}
                            >
                              {conn.sniffed_host
                                ? `${conn.sniffed_host}:${conn.dst_port}`
                                : `${conn.dst_ip}:${conn.dst_port}`}
                            </span>
                            {conn.sniff_kind && (
                              <span className="tag tag-mono">{conn.sniff_kind}</span>
                            )}
                          </div>
                          <div className="cell-secondary mono">
                            {conn.sniffed_host
                              ? `${conn.dst_ip}:${conn.dst_port}`
                              : conn.rule_name}
                          </div>
                        </td>
                        <td>
                          <span className="tag tag-mono">{conn.protocol.toUpperCase()}</span>
                        </td>
                        <td>
                          <div className="cell-primary">
                            {conn.proxy_name ??
                              (conn.action === "block"
                                ? t("connections.actionBlocked")
                                : t("connections.actionDirect"))}
                          </div>
                          <div className="cell-secondary" title={conn.rule_name}>
                            {conn.rule_name.replace(/^Preset:\s*/i, "")}
                          </div>
                        </td>
                        <td className="mono">
                          <div>TX {formatBytes(conn.bytes_tx)}</div>
                          <div className="cell-secondary">RX {formatBytes(conn.bytes_rx)}</div>
                        </td>
                        <td>
                          <span className="tag-state">
                            <span className={`dot ${dotClass}`} />
                            <span>
                              {conn.state === "active"
                                ? t("connections.stateLive")
                                : conn.state === "closed"
                                  ? t("connections.stateClosed")
                                  : conn.state === "blocked"
                                    ? t("connections.stateBlocked")
                                    : t("connections.stateFailed")}
                            </span>
                          </span>
                        </td>
                      </tr>
                      {isExpanded && (
                        <tr className="detail-row">
                          <td colSpan={6}>
                            <div className="detail-grid">
                              <div>
                                <div className="detail-item-label">
                                  {t("connections.detailExePath")}
                                </div>
                                <div className="detail-item-value mono">
                                  {conn.exe_path ?? t("connections.detailUnavailable")}
                                </div>
                              </div>
                              <div>
                                <div className="detail-item-label">
                                  {t("connections.detailSource")}
                                </div>
                                <div className="detail-item-value mono">{conn.src_addr}</div>
                              </div>
                              <div>
                                <div className="detail-item-label">
                                  {t("connections.detailRule")}
                                </div>
                                <div className="detail-item-value">{conn.rule_name}</div>
                              </div>
                              {conn.sniffed_host && (
                                <div style={{ gridColumn: "1 / -1" }}>
                                  <div className="detail-item-label">
                                    {t("connections.detailQuickActions")}
                                  </div>
                                  <div className="detail-actions">
                                    {hostBlocked ? (
                                      <span className="tag">
                                        <span className="dot dot-err" aria-hidden="true" />
                                        <span>{t("connections.domainAlreadyBlocked")}</span>
                                      </span>
                                    ) : (
                                      <>
                                        <button
                                          type="button"
                                          className="btn btn-sm btn-outline"
                                          disabled={saving}
                                          onClick={(e) => {
                                            e.stopPropagation();
                                            onBlockDomain(conn.sniffed_host!);
                                          }}
                                        >
                                          <BanIcon size={12} />
                                          <span className="mono">
                                            {t("connections.blockExactDomain", {
                                              domain: conn.sniffed_host,
                                            })}
                                          </span>
                                        </button>
                                        {wildcardPat && wildcardPat !== conn.sniffed_host && (
                                          <button
                                            type="button"
                                            className="btn btn-sm btn-ghost"
                                            disabled={saving}
                                            onClick={(e) => {
                                              e.stopPropagation();
                                              onBlockDomain(wildcardPat);
                                            }}
                                          >
                                            <BanIcon size={12} />
                                            <span className="mono">
                                              {t("connections.blockWildcardDomain", {
                                                pattern: wildcardPat,
                                              })}
                                            </span>
                                          </button>
                                        )}
                                      </>
                                    )}
                                  </div>
                                </div>
                              )}
                              {conn.error && (
                                <div style={{ gridColumn: "1 / -1" }}>
                                  <div className="detail-item-label">
                                    {t("connections.detailError")}
                                  </div>
                                  <div
                                    className="detail-item-value"
                                    style={{ color: "var(--status-err)" }}
                                  >
                                    {conn.error}
                                  </div>
                                </div>
                              )}
                            </div>
                          </td>
                        </tr>
                      )}
                    </Fragment>
                  );
                })}
              </tbody>
            </table>

            <div className="pagination-bar">
              <div className="pagination-info">
                <span>
                  {t("connections.paginationSummary", {
                    start: pageStartIdx + 1,
                    end: Math.min(pageStartIdx + pageSize, totalCount),
                    total: totalCount,
                  })}
                </span>
                <select
                  className="select pagination-size-select"
                  value={pageSize}
                  aria-label={t("connections.pageSizeAria")}
                  onChange={(e) => setPageSize(Number(e.target.value))}
                >
                  {PAGE_SIZES.map((sz) => (
                    <option key={sz} value={sz}>
                      {t("connections.perPage", { count: sz })}
                    </option>
                  ))}
                </select>
              </div>

              {totalPages > 1 && (
                <div className="pagination-controls">
                  <button
                    type="button"
                    className="page-btn"
                    disabled={safePage <= 1}
                    aria-label={t("connections.prevPage")}
                    onClick={() => setPage((p) => Math.max(1, p - 1))}
                  >
                    <ChevronLeftIcon size={13} />
                  </button>

                  {buildPageNumbers(safePage, totalPages).map((item, idx) =>
                    item === "ellipsis" ? (
                      <span key={`e-${idx}`} className="page-ellipsis">
                        ...
                      </span>
                    ) : (
                      <button
                        key={item}
                        type="button"
                        className={`page-btn ${item === safePage ? "active" : ""}`}
                        aria-current={item === safePage ? "page" : undefined}
                        onClick={() => setPage(item)}
                      >
                        {item}
                      </button>
                    )
                  )}

                  <button
                    type="button"
                    className="page-btn"
                    disabled={safePage >= totalPages}
                    aria-label={t("connections.nextPage")}
                    onClick={() => setPage((p) => Math.min(totalPages, p + 1))}
                  >
                    <ChevronRightIcon size={13} />
                  </button>
                </div>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
