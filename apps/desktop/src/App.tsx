import { useCallback, useEffect, useRef, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { Titlebar } from "./components/Titlebar";
import { api } from "./lib/api";
import { domainPatternMatches } from "./lib/connections";
import { useI18n, type MessageKey } from "./lib/i18n";
import {
  classifyDriverReadiness,
  classifyRuntimeError,
  deriveRunMode,
  needsDriverRecovery,
} from "./lib/mode";
import { useTheme, type ThemeMode } from "./lib/theme";
import { buildTrayTooltip, extractFirstProxyAddress } from "./lib/tray";
import type {
  Config,
  ConnectionRecord,
  ConnectionsViewState,
  DaemonStatus,
  EngineEvent,
  LogEntry,
  NavTab,
  PresetInfo,
  ProxyRunMode,
  RuleConfig,
  ThroughputPoint,
  UpdateCheckReport,
} from "./lib/types";
import { ApplicationsView } from "./views/ApplicationsView";
import { ConnectionsView } from "./views/ConnectionsView";
import { DiagnosticsView } from "./views/DiagnosticsView";
import { ProxiesView } from "./views/ProxiesView";

const TAB_LABEL_KEYS: Record<NavTab, MessageKey> = {
  connections: "nav.connections",
  applications: "nav.applications",
  proxies: "nav.proxies",
  diagnostics: "nav.diagnostics",
};

const MAX_SPARKLINE_POINTS = 48;

function initialTab(): NavTab {
  if (typeof window !== "undefined") {
    const tab = new URLSearchParams(window.location.search).get("tab");
    if (
      tab === "connections" ||
      tab === "applications" ||
      tab === "proxies" ||
      tab === "diagnostics"
    ) {
      return tab;
    }
  }
  return "connections";
}

const DEFAULT_CONN_STATE: ConnectionsViewState = {
  selectedProcess: "all",
  query: "",
  filter: "all",
  sortKey: "time",
  sortDir: "desc",
  pageSize: 25,
  page: 1,
  expandedId: null,
  viewMode: "streams",
  summaryGroup: "process",
};

export function App() {
  const { locale, t } = useI18n();
  const { setMode: setThemeMode } = useTheme();
  const [activeTab, setActiveTab] = useState<NavTab>(initialTab);
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [configPath, setConfigPath] = useState("");
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const [connections, setConnections] = useState<ConnectionRecord[]>([]);
  const [logs, setLogs] = useState<LogEntry[]>([]);
  const [throughputHistory, setThroughputHistory] = useState<ThroughputPoint[]>([]);
  const [connViewState, setConnViewState] = useState<ConnectionsViewState>(DEFAULT_CONN_STATE);
  const [paused, setPaused] = useState(false);
  const [switchingMode, setSwitchingMode] = useState<ProxyRunMode | null>(null);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ text: string; isError: boolean } | null>(null);
  const [updateReport, setUpdateReport] = useState<UpdateCheckReport | null>(null);
  const [checkingUpdates, setCheckingUpdates] = useState(false);
  const [updateError, setUpdateError] = useState<string | null>(null);

  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const themeBootstrappedRef = useRef(false);
  const startupUpdateCheckedRef = useRef(false);

  const formatRuntimeError = useCallback(
    (raw: string): string => {
      const code = classifyRuntimeError(raw);
      if (code === "elevation_cancelled") return t("mode.errElevationCancelled");
      if (code === "service_cancelled") return t("mode.errServiceCancelled");
      if (code === "service_pipe_timeout") return t("mode.errServiceTimeout");
      if (code === "access_denied") return t("mode.errAccessDenied");
      return raw;
    },
    [t]
  );

  const flashNotice = useCallback((text: string, isError = false) => {
    setNotice({ text, isError });
  }, []);

  useEffect(() => {
    if (!notice || notice.isError) return;
    const timer = window.setTimeout(() => setNotice(null), 3200);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const refreshAll = useCallback(async () => {
    try {
      const [st, cfgSnap, conns, presetList, logList] = await Promise.all([
        api.getStatus(),
        api.getConfig(),
        api.getConnections(),
        api.listPresets(),
        api.getLogs(undefined, 200),
      ]);
      setStatus(st);
      setConfig(cfgSnap.config);
      setConfigPath(cfgSnap.path);
      setConnections(conns);
      setPresets(presetList);
      setLogs(logList);
      if (!themeBootstrappedRef.current) {
        themeBootstrappedRef.current = true;
        const savedTheme = cfgSnap.config.general.theme;
        if (
          savedTheme === "system" ||
          savedTheme === "light" ||
          savedTheme === "dark"
        ) {
          const hasUrlTheme = new URLSearchParams(window.location.search).has("theme");
          let hasStoredTheme = false;
          try {
            hasStoredTheme = Boolean(window.localStorage.getItem("bypath-theme"));
          } catch {
            hasStoredTheme = false;
          }
          if (!hasUrlTheme && !hasStoredTheme && savedTheme !== "system") {
            setThemeMode(savedTheme as ThemeMode);
          }
        }
      }
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      flashNotice(formatRuntimeError(raw), true);
    }
  }, [flashNotice, formatRuntimeError, setThemeMode]);

  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  const handleCheckUpdates = useCallback(
    async (silent = false) => {
      setCheckingUpdates(true);
      if (!silent) {
        setUpdateError(null);
      }
      try {
        const report = await api.checkForUpdates();
        setUpdateReport(report);
        setUpdateError(null);
        try {
          window.localStorage.setItem("bypath-last-update-check", String(Date.now()));
        } catch {
          // Ignore storage failures.
        }
        if (!silent) {
          if (report.update_available) {
            flashNotice(t("diag.toastUpdateAvailable", { tag: report.latest_tag }), false);
          } else {
            flashNotice(t("diag.toastUpToDate", { version: report.current_version }), false);
          }
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        if (!silent) {
          setUpdateError(msg);
          flashNotice(msg, true);
        }
      } finally {
        setCheckingUpdates(false);
      }
    },
    [flashNotice, t]
  );

  // Silent startup update check (throttled to once per 12h, respects general.auto_check_updates).
  useEffect(() => {
    if (!config || startupUpdateCheckedRef.current) return;
    startupUpdateCheckedRef.current = true;
    if (config.general.auto_check_updates === false) return;
    const TWELVE_HOURS_MS = 12 * 60 * 60 * 1000;
    let lastChecked = 0;
    try {
      lastChecked = Number(window.localStorage.getItem("bypath-last-update-check") ?? "0");
    } catch {
      lastChecked = 0;
    }
    if (Number.isFinite(lastChecked) && Date.now() - lastChecked < TWELVE_HOURS_MS) {
      return;
    }
    const timer = window.setTimeout(() => {
      void handleCheckUpdates(true);
    }, 2500);
    return () => window.clearTimeout(timer);
  }, [config, handleCheckUpdates]);

  // Sync localized tray tooltip and close-to-tray behavior whenever state changes.
  const firstProxyAddress = extractFirstProxyAddress(config);
  const runMode = deriveRunMode(status);

  useEffect(() => {
    const running = runMode !== "off";
    const proxyLabel = firstProxyAddress ?? "-";
    const runningLabel =
      runMode === "background"
        ? t("tray.statusBackground", { proxy: proxyLabel })
        : t("tray.statusForeground", { proxy: proxyLabel });
    const tooltip = buildTrayTooltip(running, runningLabel, t("tray.statusOff"));
    void api.setTrayState({
      tooltip,
      minimize_to_tray: config?.general.minimize_to_tray ?? true,
    });
  }, [locale, runMode, firstProxyAddress, config?.general.minimize_to_tray, t]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void api
      .subscribeNavigate((tab) => {
        setActiveTab(tab);
      })
      .then((fn) => {
        unlisten = fn;
      });
    return () => {
      unlisten?.();
    };
  }, []);

  // Global keyboard shortcuts (Ctrl+1..4 tabs, Ctrl+K or / search, Space pause, Esc collapse)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const inInput =
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT" ||
          target.isContentEditable);

      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey) {
        if (e.key === "1") {
          e.preventDefault();
          setActiveTab("connections");
          return;
        }
        if (e.key === "2") {
          e.preventDefault();
          setActiveTab("applications");
          return;
        }
        if (e.key === "3") {
          e.preventDefault();
          setActiveTab("proxies");
          return;
        }
        if (e.key === "4") {
          e.preventDefault();
          setActiveTab("diagnostics");
          return;
        }
        if (e.key.toLowerCase() === "k") {
          e.preventDefault();
          setActiveTab("connections");
          window.setTimeout(() => searchInputRef.current?.focus(), 20);
          return;
        }
      }

      if (!inInput && e.key === "/" && activeTab === "connections") {
        e.preventDefault();
        searchInputRef.current?.focus();
      } else if (!inInput && e.key === " " && activeTab === "connections") {
        e.preventDefault();
        setPaused((v) => !v);
      } else if (e.key === "Escape") {
        setConnViewState((prev) =>
          prev.expandedId !== null ? { ...prev, expandedId: null } : prev
        );
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [activeTab]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void api
      .subscribeEvents(
        (evt: EngineEvent) => {
          switch (evt.type) {
            case "stats_tick":
              setStatus((prev) => (prev ? { ...prev, stats: evt.stats } : prev));
              setThroughputHistory((prev) => [
                ...prev.slice(-(MAX_SPARKLINE_POINTS - 1)),
                {
                  tx: evt.stats.tx_bytes_per_sec,
                  rx: evt.stats.rx_bytes_per_sec,
                },
              ]);
              break;
            case "connection_upsert":
              if (pausedRef.current) return;
              setConnections((prev) => {
                const idx = prev.findIndex((c) => c.id === evt.connection.id);
                if (idx >= 0) {
                  const copy = [...prev];
                  copy[idx] = evt.connection;
                  return copy;
                }
                return [evt.connection, ...prev].slice(0, 500);
              });
              break;
            case "connections_cleared":
              setConnections((prev) => prev.filter((c) => c.state === "active"));
              break;
            case "state_changed":
              setStatus((prev) =>
                prev
                  ? {
                      ...prev,
                      engine_running: evt.running,
                      last_error: evt.error,
                      stats: { ...prev.stats, running: evt.running },
                    }
                  : prev
              );
              if (evt.error) {
                flashNotice(formatRuntimeError(evt.error), true);
              }
              break;
            case "config_reloaded":
              void refreshAll();
              break;
            case "log_entry":
              setLogs((prev) => [evt.entry, ...prev].slice(0, 300));
              break;
          }
        },
        () => {
          void refreshAll();
        }
      )
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      unlisten?.();
    };
  }, [flashNotice, formatRuntimeError, refreshAll]);

  const handleSelectMode = async (target: ProxyRunMode) => {
    setSwitchingMode(target);
    setNotice(null);
    try {
      const res = await api.setRunMode(target);
      setStatus(res.status);
      if (res.elevating) {
        flashNotice(t("mode.toastForegroundElevating"), false);
        return;
      }
      if (target === "off") {
        flashNotice(t("mode.toastOff"), false);
      } else if (target === "foreground") {
        flashNotice(t("mode.toastForeground"), false);
      } else {
        flashNotice(t("mode.toastBackground"), false);
      }
      await refreshAll();
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      flashNotice(formatRuntimeError(raw), true);
    } finally {
      setSwitchingMode(null);
    }
  };

  const handleSaveConfig = async (next: Config, toast?: string) => {
    setSaving(true);
    setNotice(null);
    try {
      await api.saveConfig(next, true);
      setConfig(next);
      if (toast) flashNotice(toast, false);
      const st = await api.getStatus();
      setStatus(st);
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      flashNotice(formatRuntimeError(raw), true);
    } finally {
      setSaving(false);
    }
  };

  const handleLocateDriverDir = async () => {
    if (!config) return;
    try {
      const picked = await api.pickDirectory(t("diag.wdDirTitle"));
      if (!picked) return;
      await handleSaveConfig(
        {
          ...config,
          general: { ...config.general, windivert_dir: picked },
        },
        t("diag.toastWdSaved")
      );
      await refreshAll();
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      flashNotice(formatRuntimeError(raw), true);
    }
  };

  const handleClearClosed = async () => {
    try {
      await api.clearConnections();
      setConnections((prev) => prev.filter((c) => c.state === "active"));
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      flashNotice(formatRuntimeError(raw), true);
    }
  };

  const handleBlockDomain = async (rawPattern: string) => {
    if (!config) return;
    const pattern = rawPattern.trim().replace(/\.+$/, "").toLowerCase();
    if (!pattern) return;

    const nextRules = config.rules.map((r) => ({ ...r, domains: [...(r.domains ?? [])] }));
    const existingIdx = nextRules.findIndex(
      (r) =>
        r.action === "block" &&
        r.process_names.length === 0 &&
        r.path_prefixes.length === 0 &&
        r.path_contains.length === 0
    );

    if (existingIdx >= 0) {
      const target = nextRules[existingIdx];
      if (!target.domains.some((d) => d.toLowerCase() === pattern)) {
        target.domains.push(pattern);
      }
      target.enabled = true;
    } else {
      nextRules.push({
        name: "Blocked Domains",
        enabled: true,
        process_names: [],
        path_prefixes: [],
        path_contains: [],
        include_children: true,
        dst_cidrs: [],
        dst_ports: [],
        protocols: [],
        domains: [pattern],
        action: "block",
        proxy: null,
      });
    }

    await handleSaveConfig(
      { ...config, rules: nextRules },
      t("connections.toastDomainBlocked", { pattern })
    );
  };

  const handleDirectDomain = async (rawPattern: string) => {
    if (!config) return;
    const pattern = rawPattern.trim().replace(/\.+$/, "").toLowerCase();
    if (!pattern) return;

    const nextRules = config.rules.map((r) => ({ ...r, domains: [...(r.domains ?? [])] }));
    const existingIdx = nextRules.findIndex(
      (r) =>
        r.action === "direct" &&
        r.process_names.length === 0 &&
        r.path_prefixes.length === 0 &&
        r.path_contains.length === 0 &&
        r.domains.length > 0
    );

    if (existingIdx >= 0) {
      const target = nextRules[existingIdx];
      if (!target.domains.some((d) => d.toLowerCase() === pattern)) {
        target.domains.push(pattern);
      }
      target.enabled = true;
    } else {
      nextRules.unshift({
        name: "Direct Domains",
        enabled: true,
        process_names: [],
        path_prefixes: [],
        path_contains: [],
        include_children: true,
        dst_cidrs: [],
        dst_ports: [],
        protocols: [],
        domains: [pattern],
        action: "direct",
        proxy: null,
      });
    }

    await handleSaveConfig(
      { ...config, rules: nextRules },
      t("connections.toastDomainDirect", { pattern })
    );
  };

  const handleUnblockDomain = async (host: string) => {
    if (!config) return;
    const nextRules: RuleConfig[] = [];
    for (const rule of config.rules) {
      if (rule.action === "block" && (rule.domains ?? []).length > 0) {
        const remaining = rule.domains.filter((pat) => !domainPatternMatches(pat, host));
        if (remaining.length > 0) {
          nextRules.push({ ...rule, domains: remaining });
        }
      } else {
        nextRules.push(rule);
      }
    }
    await handleSaveConfig(
      { ...config, rules: nextRules },
      t("connections.toastDomainUnblocked", { domain: host })
    );
  };

  const driverReadiness = classifyDriverReadiness(status);
  const showDriverRecoveryBanner = needsDriverRecovery(status);

  return (
    <div className="app-frame">
      <Titlebar
        sectionLabel={t(TAB_LABEL_KEYS[activeTab])}
        status={status}
        firstProxyAddress={firstProxyAddress}
        updateAvailableTag={updateReport?.update_available ? updateReport.latest_tag : null}
        onOpenUpdates={() => setActiveTab("diagnostics")}
        onRefresh={() => void refreshAll()}
      />

      <div className="app-body">
        <Sidebar
          activeTab={activeTab}
          onSelectTab={setActiveTab}
          status={status}
          config={config}
          switchingMode={switchingMode}
          onSelectMode={(target) => void handleSelectMode(target)}
        />

        <main className="main-area">
          <div className="main-scroll">
            {notice && (
              <div
                className={`notice-bar ${notice.isError ? "error" : ""}`}
                style={{ maxWidth: 1080, margin: "0 auto 16px" }}
                role={notice.isError ? "alert" : "status"}
              >
                <div className="notice-bar-content">
                  <span className={`dot ${notice.isError ? "dot-err" : "dot-ok"}`} />
                  <span>{notice.text}</span>
                </div>
                <button
                  type="button"
                  className="btn btn-sm btn-ghost"
                  onClick={() => setNotice(null)}
                >
                  {t("topbar.dismiss")}
                </button>
              </div>
            )}

            {showDriverRecoveryBanner && status && (
              <section className="recovery-banner" aria-label={t("recovery.title")}>
                <div className="recovery-banner-body">
                  <div className="recovery-banner-title">
                    <span className="dot dot-err" aria-hidden="true" />
                    <span>{t("recovery.title")}</span>
                  </div>
                  <p className="recovery-banner-desc">
                    {driverReadiness === "blocked_by_security"
                      ? t("recovery.descBlocked")
                      : driverReadiness === "missing"
                        ? t("recovery.descMissing")
                        : t("recovery.descError")}
                  </p>
                </div>
                <div className="toolbar-group">
                  <button
                    type="button"
                    className="btn btn-sm btn-outline"
                    onClick={() => void handleLocateDriverDir()}
                  >
                    {t("setup.pickDriverDir")}
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => void api.openWindowsSecurity()}
                  >
                    {t("setup.openSecurity")}
                  </button>
                </div>
              </section>
            )}

            {activeTab === "connections" && (
              <ConnectionsView
                status={status}
                config={config}
                connections={connections}
                throughputHistory={throughputHistory}
                viewState={connViewState}
                onChangeViewState={setConnViewState}
                searchInputRef={searchInputRef}
                paused={paused}
                saving={saving}
                switchingMode={switchingMode}
                onTogglePause={() => setPaused((v) => !v)}
                onClearClosed={() => void handleClearClosed()}
                onSelectMode={(target) => void handleSelectMode(target)}
                onOpenDiagnostics={() => setActiveTab("diagnostics")}
                onBlockDomain={(pattern) => void handleBlockDomain(pattern)}
                onDirectDomain={(pattern) => void handleDirectDomain(pattern)}
                onUnblockDomain={(host) => void handleUnblockDomain(host)}
                onNotify={(msg) => flashNotice(msg, false)}
              />
            )}

            {activeTab === "applications" && config && (
              <ApplicationsView
                config={config}
                presets={presets}
                saving={saving}
                onSaveConfig={handleSaveConfig}
              />
            )}

            {activeTab === "proxies" && config && (
              <ProxiesView
                config={config}
                saving={saving}
                onSaveConfig={handleSaveConfig}
                onNotify={(msg, isErr) => flashNotice(msg, Boolean(isErr))}
              />
            )}

            {activeTab === "diagnostics" && config && (
              <DiagnosticsView
                status={status}
                config={config}
                configPath={configPath}
                logs={logs}
                saving={saving}
                switchingMode={switchingMode}
                updateReport={updateReport}
                checkingUpdates={checkingUpdates}
                updateError={updateError}
                onSelectMode={(target) => void handleSelectMode(target)}
                onCheckUpdates={() => void handleCheckUpdates(false)}
                onSaveConfig={handleSaveConfig}
                onRefreshStatus={refreshAll}
                onNotify={(msg, isErr) => flashNotice(msg, Boolean(isErr))}
              />
            )}
          </div>
        </main>
      </div>
    </div>
  );
}
