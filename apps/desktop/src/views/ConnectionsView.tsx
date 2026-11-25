import { Fragment, useEffect, useMemo, type Dispatch, type RefObject, type SetStateAction } from "react";
import {
  ActivityIcon,
  BanIcon,
  BypassIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CopyIcon,
  DownloadIcon,
  PauseIcon,
  PlayIcon,
  SearchIcon,
  SortAscIcon,
  SortDescIcon,
  SortNeutralIcon,
  TrashIcon,
} from "../components/Icons";
import { Select } from "../components/Select";
import {
  aggregateConnections,
  buildPageNumbers,
  connectionsToCsv,
  defaultSortDir,
  filterAndSortConnections,
  isDomainBlockedByConfig,
  isDomainDirectByConfig,
  wildcardSuggestion,
} from "../lib/connections";
import { formatBytes, formatDuration, formatSpeed, formatTime, formatUptime } from "../lib/format";
import { useI18n } from "../lib/i18n";
import type {
  Config,
  ConnectionRecord,
  ConnectionsViewState,
  DaemonStatus,
  SortKey,
  StateFilter,
  ThroughputPoint,
} from "../lib/types";

interface ConnectionsViewProps {
  status: DaemonStatus | null;
  config: Config | null;
  connections: ConnectionRecord[];
  throughputHistory: ThroughputPoint[];
  viewState: ConnectionsViewState;
  onChangeViewState: Dispatch<SetStateAction<ConnectionsViewState>>;
  searchInputRef: RefObject<HTMLInputElement | null>;
  paused: boolean;
  saving: boolean;
  onTogglePause: () => void;
  onClearClosed: () => void;
  onStartEngine: () => void;
  onOpenDiagnostics: () => void;
  onBlockDomain: (pattern: string) => void;
  onDirectDomain: (pattern: string) => void;
  onUnblockDomain: (host: string) => void;
  onNotify: (message: string) => void;
}

const PAGE_SIZES = [25, 50, 100] as const;

function Sparkline({ values }: { values: number[] }) {
  const pts = useMemo(() => {
    const series = values.length > 1 ? values : [0, 0];
    const max = Math.max(1, ...series);
    const w = 68;
    const h = 22;
    const pad = 2;
    return series
      .map((v, idx) => {
        const x = (idx / Math.max(1, series.length - 1)) * w;
        const y = h - pad - (v / max) * (h - pad * 2);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");
  }, [values]);

  return (
    <svg className="sparkline-svg" viewBox="0 0 68 22" fill="none" aria-hidden="true">
      <polyline
        points={pts}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

export function ConnectionsView({
  status,
  config,
  connections,
  throughputHistory,
  viewState,
  onChangeViewState,
  searchInputRef,
  paused,
  saving,
  onTogglePause,
  onClearClosed,
  onStartEngine,
  onOpenDiagnostics,
  onBlockDomain,
  onDirectDomain,
  onUnblockDomain,
  onNotify,
}: ConnectionsViewProps) {
  const { t } = useI18n();
  const {
    selectedProcess,
    query,
    filter,
    sortKey,
    sortDir,
    pageSize,
    page,
    expandedId,
    viewMode,
    summaryGroup,
  } = viewState;

  const patchState = (partial: Partial<ConnectionsViewState>) => {
    onChangeViewState((prev) => ({ ...prev, ...partial }));
  };

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
      onChangeViewState((prev) => ({ ...prev, selectedProcess: "all", page: 1 }));
    }
  }, [processTabs, selectedProcess, onChangeViewState]);

  const filteredAndSorted = useMemo(
    () =>
      filterAndSortConnections(
        connections,
        selectedProcess,
        query,
        filter,
        sortKey,
        sortDir
      ),
    [connections, selectedProcess, query, filter, sortKey, sortDir]
  );

  const summaryRows = useMemo(
    () => aggregateConnections(filteredAndSorted, summaryGroup),
    [filteredAndSorted, summaryGroup]
  );

  const maxSummaryTraffic = useMemo(
    () => Math.max(1, ...summaryRows.map((r) => r.bytesTx + r.bytesRx)),
    [summaryRows]
  );

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
      patchState({ sortDir: sortDir === "asc" ? "desc" : "asc", page: 1 });
    } else {
      patchState({ sortKey: key, sortDir: defaultSortDir(key), page: 1 });
    }
  };

  const renderSortIcon = (key: SortKey) => {
    if (sortKey !== key) {
      return <SortNeutralIcon size={12} className="sort-icon-muted" />;
    }
    return sortDir === "asc" ? <SortAscIcon size={12} /> : <SortDescIcon size={12} />;
  };

  const copyText = (text: string) => {
    void navigator.clipboard.writeText(text).then(() => {
      onNotify(t("connections.copied"));
    });
  };

  const handleExportCsv = () => {
    if (filteredAndSorted.length === 0) return;
    const csv = connectionsToCsv(filteredAndSorted);
    const blob = new Blob([csv], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `bypath-connections-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, "-")}.csv`;
    a.click();
    URL.revokeObjectURL(url);
    onNotify(t("connections.exportedCsv", { count: filteredAndSorted.length }));
  };

  const running = status?.engine_running ?? false;
  const stats = status?.stats;
  const dllLoaded = Boolean(status?.windivert_dll_path);
  const accessDenied =
    status?.windivert_error_kind === "access_denied" ||
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

  const txSeries = useMemo(() => throughputHistory.map((p) => p.tx), [throughputHistory]);
  const rxSeries = useMemo(() => throughputHistory.map((p) => p.rx), [throughputHistory]);

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
          <div className="telemetry-val-row">
            <span className="telemetry-value">{formatSpeed(stats?.tx_bytes_per_sec ?? 0)}</span>
            <Sparkline values={txSeries} />
          </div>
          <span className="telemetry-sub">
            {t("connections.total", { size: formatBytes(stats?.total_bytes_tx ?? 0) })}
          </span>
        </div>

        <div className="telemetry-cell">
          <span className="telemetry-label">{t("connections.download")}</span>
          <div className="telemetry-val-row">
            <span className="telemetry-value">{formatSpeed(stats?.rx_bytes_per_sec ?? 0)}</span>
            <Sparkline values={rxSeries} />
          </div>
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
            onClick={() => patchState({ selectedProcess: "all", page: 1 })}
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
              onClick={() => patchState({ selectedProcess: proc.name, page: 1 })}
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
            ref={searchInputRef}
            className="input"
            type="search"
            value={query}
            onChange={(e) => patchState({ query: e.target.value, page: 1 })}
            placeholder={t("connections.searchPlaceholder")}
            aria-label={t("connections.searchAria")}
          />
        </div>

        <div className="toolbar-group">
          <div className="segmented" role="tablist" aria-label={t("connections.viewStreams")}>
            <button
              type="button"
              role="tab"
              aria-selected={viewMode === "streams"}
              className={`segmented-btn ${viewMode === "streams" ? "active" : ""}`}
              onClick={() => patchState({ viewMode: "streams" })}
            >
              {t("connections.viewStreams")}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={viewMode === "summary"}
              className={`segmented-btn ${viewMode === "summary" ? "active" : ""}`}
              onClick={() => patchState({ viewMode: "summary" })}
            >
              {t("connections.viewSummary")}
            </button>
          </div>

          <div className="segmented" role="tablist" aria-label={t("connections.filterAria")}>
            {(["all", "active", "blocked"] as StateFilter[]).map((key) => (
              <button
                key={key}
                type="button"
                role="tab"
                aria-selected={filter === key}
                className={`segmented-btn ${filter === key ? "active" : ""}`}
                onClick={() => patchState({ filter: key, page: 1 })}
              >
                {key === "all"
                  ? t("connections.filterAll")
                  : key === "active"
                    ? t("connections.filterActive")
                    : t("connections.filterBlocked")}
              </button>
            ))}
          </div>

          {viewMode === "streams" ? (
            <div className="sort-control">
              <Select<SortKey>
                size="sm"
                value={sortKey}
                ariaLabel={t("connections.sortAria")}
                onChange={(next) =>
                  patchState({ sortKey: next, sortDir: defaultSortDir(next), page: 1 })
                }
                options={[
                  { value: "time", label: t("connections.sortTime") },
                  { value: "transfer", label: t("connections.sortTransfer") },
                  { value: "tx", label: t("connections.sortTx") },
                  { value: "rx", label: t("connections.sortRx") },
                  { value: "process", label: t("connections.sortProcess") },
                  { value: "destination", label: t("connections.sortDestination") },
                  { value: "state", label: t("connections.sortState") },
                  { value: "proto", label: t("connections.sortProto") },
                ]}
              />
              <button
                type="button"
                className="btn btn-sm btn-outline sort-dir-btn"
                aria-label={t("connections.sortDirectionAria")}
                title={t("connections.sortDirectionAria")}
                onClick={() => patchState({ sortDir: sortDir === "asc" ? "desc" : "asc", page: 1 })}
              >
                {sortDir === "asc" ? <SortAscIcon size={13} /> : <SortDescIcon size={13} />}
              </button>
            </div>
          ) : (
            <div className="segmented" role="tablist" aria-label={t("connections.viewSummary")}>
              <button
                type="button"
                role="tab"
                aria-selected={summaryGroup === "process"}
                className={`segmented-btn ${summaryGroup === "process" ? "active" : ""}`}
                onClick={() => patchState({ summaryGroup: "process" })}
              >
                {t("connections.groupByProcess")}
              </button>
              <button
                type="button"
                role="tab"
                aria-selected={summaryGroup === "domain"}
                className={`segmented-btn ${summaryGroup === "domain" ? "active" : ""}`}
                onClick={() => patchState({ summaryGroup: "domain" })}
              >
                {t("connections.groupByDomain")}
              </button>
            </div>
          )}

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
            onClick={handleExportCsv}
            disabled={filteredAndSorted.length === 0}
            title={t("connections.exportCsv")}
          >
            <DownloadIcon size={13} />
            <span>{t("connections.exportCsv")}</span>
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
        ) : viewMode === "summary" ? (
          <table className="data-table">
            <thead>
              <tr>
                <th style={{ width: "42%" }}>{t("connections.summaryColName")}</th>
                <th style={{ width: "14%" }}>{t("connections.summaryColStreams")}</th>
                <th style={{ width: "26%" }}>{t("connections.summaryColTraffic")}</th>
                <th style={{ width: "18%" }}>{t("connections.summaryColStatus")}</th>
              </tr>
            </thead>
            <tbody>
              {summaryRows.map((row) => {
                const totalBytes = row.bytesTx + row.bytesRx;
                const pct = Math.max(4, Math.round((totalBytes / maxSummaryTraffic) * 100));
                const blocked = row.isDomain ? isDomainBlockedByConfig(config, row.key) : false;
                return (
                  <tr key={row.key}>
                    <td>
                      <div className={`cell-primary ${row.isDomain ? "mono" : ""}`} title={row.key}>
                        {row.key}
                      </div>
                      <div className="summary-track">
                        <div className="summary-fill" style={{ width: `${pct}%` }} />
                      </div>
                    </td>
                    <td className="mono">{row.count}</td>
                    <td className="mono">
                      <div>{formatBytes(totalBytes)}</div>
                      <div className="cell-secondary">
                        TX {formatBytes(row.bytesTx)} / RX {formatBytes(row.bytesRx)}
                      </div>
                    </td>
                    <td>
                      <div className="cell-secondary">
                        {t("connections.summaryActiveBlocked", {
                          active: row.active,
                          blocked: row.blocked,
                        })}
                      </div>
                      {row.isDomain && !blocked && (
                        <button
                          type="button"
                          className="btn btn-sm btn-ghost"
                          style={{ height: 22, padding: "0 6px", marginTop: 4, fontSize: 11.5 }}
                          disabled={saving}
                          onClick={() => onBlockDomain(row.key)}
                        >
                          <BanIcon size={11} />
                          <span>{t("connections.stateBlocked")}</span>
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ) : (
          <>
            <table className="data-table">
              <thead>
                <tr>
                  <th style={{ width: "22%" }}>
                    <button
                      type="button"
                      className={`th-sort-btn ${sortKey === "process" ? "active" : ""}`}
                      onClick={() => handleHeaderSort("process")}
                    >
                      <span>{t("connections.colProcess")}</span>
                      {renderSortIcon("process")}
                    </button>
                  </th>
                  <th style={{ width: "33%" }}>
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
                  const hostDirect = isDomainDirectByConfig(config, conn.sniffed_host);
                  const wildcardPat = conn.sniffed_host
                    ? wildcardSuggestion(conn.sniffed_host)
                    : null;
                  const targetDisplay = conn.sniffed_host
                    ? `${conn.sniffed_host}:${conn.dst_port}`
                    : `${conn.dst_ip}:${conn.dst_port}`;

                  return (
                    <Fragment key={conn.id}>
                      <tr
                        className={`clickable ${isExpanded ? "selected-row" : ""}`}
                        onClick={() => patchState({ expandedId: isExpanded ? null : conn.id })}
                      >
                        <td>
                          <div className="cell-primary" title={conn.process_name}>
                            {conn.process_name}
                          </div>
                          <div className="cell-secondary mono">
                            {conn.pid ? `PID ${conn.pid}` : t("connections.unknownPid")} /{" "}
                            {formatTime(conn.started_at_ms)}
                          </div>
                        </td>
                        <td>
                          <div
                            className="cell-primary mono"
                            style={{ display: "flex", alignItems: "center", gap: 6 }}
                            title={targetDisplay}
                          >
                            <span
                              style={{
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                              }}
                            >
                              {targetDisplay}
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
                                  {t("connections.detailRule")} / {t("connections.detailDuration")}
                                </div>
                                <div className="detail-item-value">
                                  {conn.rule_name} ({formatDuration(conn.started_at_ms, conn.ended_at_ms)})
                                </div>
                              </div>

                              <div style={{ gridColumn: "1 / -1" }}>
                                <div className="detail-item-label">
                                  {t("connections.detailQuickActions")}
                                </div>
                                <div className="detail-actions">
                                  <button
                                    type="button"
                                    className="btn btn-sm btn-outline"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      copyText(conn.sniffed_host ?? `${conn.dst_ip}:${conn.dst_port}`);
                                    }}
                                  >
                                    <CopyIcon size={12} />
                                    <span>{t("connections.copyTarget")}</span>
                                  </button>

                                  {conn.exe_path && (
                                    <button
                                      type="button"
                                      className="btn btn-sm btn-ghost"
                                      onClick={(e) => {
                                        e.stopPropagation();
                                        copyText(conn.exe_path!);
                                      }}
                                    >
                                      <CopyIcon size={12} />
                                      <span>{t("connections.copyPath")}</span>
                                    </button>
                                  )}

                                  {conn.sniffed_host && (
                                    <>
                                      {hostDirect ? (
                                        <span className="tag">
                                          <span className="dot dot-ok" aria-hidden="true" />
                                          <span>{t("connections.domainAlreadyDirect")}</span>
                                        </span>
                                      ) : (
                                        <button
                                          type="button"
                                          className="btn btn-sm btn-outline"
                                          disabled={saving}
                                          onClick={(e) => {
                                            e.stopPropagation();
                                            onDirectDomain(conn.sniffed_host!);
                                          }}
                                        >
                                          <BypassIcon size={12} />
                                          <span className="mono">
                                            {t("connections.directDomain", {
                                              domain: conn.sniffed_host,
                                            })}
                                          </span>
                                        </button>
                                      )}

                                      {hostBlocked ? (
                                        <button
                                          type="button"
                                          className="btn btn-sm btn-outline"
                                          disabled={saving}
                                          onClick={(e) => {
                                            e.stopPropagation();
                                            onUnblockDomain(conn.sniffed_host!);
                                          }}
                                        >
                                          <span className="dot dot-err" aria-hidden="true" />
                                          <span className="mono">
                                            {t("connections.unblockDomain", {
                                              domain: conn.sniffed_host,
                                            })}
                                          </span>
                                        </button>
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
                                    </>
                                  )}
                                </div>
                              </div>

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
                <Select
                  size="xs"
                  value={String(pageSize)}
                  ariaLabel={t("connections.pageSizeAria")}
                  onChange={(val) => patchState({ pageSize: Number(val), page: 1 })}
                  options={PAGE_SIZES.map((sz) => ({
                    value: String(sz),
                    label: t("connections.perPage", { count: sz }),
                  }))}
                />
              </div>

              {totalPages > 1 && (
                <div className="pagination-controls">
                  <button
                    type="button"
                    className="page-btn"
                    disabled={safePage <= 1}
                    aria-label={t("connections.prevPage")}
                    onClick={() => patchState({ page: Math.max(1, safePage - 1) })}
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
                        onClick={() => patchState({ page: item })}
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
                    onClick={() => patchState({ page: Math.min(totalPages, safePage + 1) })}
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
