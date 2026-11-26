import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import {
  ActivityIcon,
  AppsIcon,
  AppWindowIcon,
  ArrowDownIcon,
  ArrowUpIcon,
  BrandMark,
  ChevronRightIcon,
  LanguageIcon,
  MonitorIcon,
  MoonIcon,
  PowerIcon,
  RouteIcon,
  ShieldIcon,
  SunIcon,
} from "./Icons";
import { ModeSwitcher } from "./ModeSwitcher";
import { api } from "../lib/api";
import { formatSpeed } from "../lib/format";
import { useI18n } from "../lib/i18n";
import { classifyDriverReadiness, classifyRuntimeError } from "../lib/mode";
import { useTheme } from "../lib/theme";
import { buildTraySummary, buildTrayTooltip } from "../lib/tray";
import type { Config, DaemonStatus, EngineEvent, NavTab, ProxyRunMode } from "../lib/types";

const TRAY_WINDOW_WIDTH = 288;

export function TrayMenu() {
  const { locale, toggleLocale, t } = useI18n();
  const { mode: themeMode, cycleMode } = useTheme();
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [switchingMode, setSwitchingMode] = useState<ProxyRunMode | null>(null);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [openSeq, setOpenSeq] = useState(0);

  const shellRef = useRef<HTMLDivElement | null>(null);
  const lastHeightRef = useRef<number>(0);

  const formatError = useCallback(
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

  const refreshState = useCallback(async () => {
    try {
      const [st, cfgSnap] = await Promise.all([api.getStatus(), api.getConfig()]);
      setStatus(st);
      setConfig(cfgSnap.config);
      setErrorMsg(null);
    } catch (err) {
      setErrorMsg(formatError(err instanceof Error ? err.message : String(err)));
    }
  }, [formatError]);

  useEffect(() => {
    void refreshState();
  }, [refreshState]);

  // Keep live metrics and state in sync with the engine and tray open events.
  useEffect(() => {
    let unlistenEvents: (() => void) | null = null;
    let unlistenOpened: (() => void) | null = null;

    void api
      .subscribeEvents(
        (evt: EngineEvent) => {
          switch (evt.type) {
            case "stats_tick":
              setStatus((prev) => (prev ? { ...prev, stats: evt.stats } : prev));
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
                setErrorMsg(formatError(evt.error));
              }
              break;
            case "config_reloaded":
              void refreshState();
              break;
          }
        },
        () => {
          void refreshState();
        }
      )
      .then((fn) => {
        unlistenEvents = fn;
      });

    void api
      .subscribeTrayOpened(() => {
        setOpenSeq((n) => n + 1);
        setErrorMsg(null);
        void refreshState();
      })
      .then((fn) => {
        unlistenOpened = fn;
      });

    return () => {
      unlistenEvents?.();
      unlistenOpened?.();
    };
  }, [formatError, refreshState]);

  // Auto-fit the transparent Tauri popup window height to exact rendered content.
  useEffect(() => {
    const el = shellRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;

    const reportSize = () => {
      const rect = el.getBoundingClientRect();
      const nextHeight = Math.ceil(rect.height);
      if (nextHeight > 0 && nextHeight !== lastHeightRef.current) {
        lastHeightRef.current = nextHeight;
        void api.resizeTrayMenu(TRAY_WINDOW_WIDTH, nextHeight);
      }
    };

    reportSize();
    const observer = new ResizeObserver(reportSize);
    observer.observe(el);
    return () => observer.disconnect();
  }, [locale, errorMsg, status?.engine_running, status?.backend_mode]);

  const summary = buildTraySummary(status, config);
  const driverReadiness = classifyDriverReadiness(status);
  const proxiesCount = config ? Object.keys(config.proxies).length : status?.proxies_count ?? 0;

  // Keep native tray hover tooltip synced even if switched directly from the tray popover.
  useEffect(() => {
    if (!status && !config) return;
    const proxyLabel = summary.proxyAddress ?? "-";
    const runningLabel =
      summary.runMode === "background"
        ? t("tray.statusBackground", { proxy: proxyLabel })
        : t("tray.statusForeground", { proxy: proxyLabel });
    const tooltip = buildTrayTooltip(summary.running, runningLabel, t("tray.statusOff"));
    void api.setTrayState({ tooltip });
  }, [locale, status, config, summary.running, summary.runMode, summary.proxyAddress, t]);

  const handleSelectMode = async (target: ProxyRunMode) => {
    setSwitchingMode(target);
    setErrorMsg(null);
    try {
      const res = await api.setRunMode(target);
      setStatus(res.status);
      if (!res.elevating) {
        await refreshState();
      }
    } catch (err) {
      setErrorMsg(formatError(err instanceof Error ? err.message : String(err)));
    } finally {
      setSwitchingMode(null);
    }
  };

  const handleOpenWindow = (tab?: NavTab) => {
    void api.openMainWindow(tab);
  };

  const handleQuit = () => {
    void api.quitApp();
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      void api.hideTrayMenu();
      return;
    }

    if (
      e.key !== "ArrowDown" &&
      e.key !== "ArrowUp" &&
      e.key !== "Home" &&
      e.key !== "End"
    ) {
      return;
    }

    const container = shellRef.current;
    if (!container) return;
    const items = Array.from(
      container.querySelectorAll<HTMLButtonElement>(
        'button[data-tray-focusable="true"]:not(:disabled)'
      )
    );
    if (items.length === 0) return;

    e.preventDefault();
    const activeEl = document.activeElement as HTMLButtonElement | null;
    const currentIdx = activeEl ? items.indexOf(activeEl) : -1;

    if (e.key === "Home") {
      items[0].focus();
      return;
    }
    if (e.key === "End") {
      items[items.length - 1].focus();
      return;
    }
    if (e.key === "ArrowDown") {
      const next = currentIdx < 0 ? 0 : (currentIdx + 1) % items.length;
      items[next].focus();
      return;
    }
    if (e.key === "ArrowUp") {
      const prev = currentIdx <= 0 ? items.length - 1 : currentIdx - 1;
      items[prev].focus();
    }
  };

  const themeLabel =
    themeMode === "dark"
      ? t("theme.dark")
      : themeMode === "light"
        ? t("theme.light")
        : t("theme.system");

  const statusDotClass = summary.running
    ? "dot-ok"
    : summary.needsAttention
      ? "dot-warn"
      : "dot-idle";

  const statusBadgeLabel =
    summary.runMode === "background"
      ? t("mode.backgroundActive")
      : summary.runMode === "foreground"
        ? t("mode.foregroundActive")
        : t("mode.offTitle");

  return (
    <div ref={shellRef} className="tray-shell" onKeyDown={handleKeyDown}>
      <div key={openSeq} className="tray-card" role="menu" aria-label={t("tray.menuAria")}>
        <div className="tray-header">
          <div className="tray-brand-row">
            <div className="tray-brand-left">
              <BrandMark size={18} />
              <span className="tray-brand-name">{t("app.name")}</span>
            </div>
            <div className="tray-status-badge">
              <span className={`dot ${statusDotClass}`} />
              <span>{statusBadgeLabel}</span>
            </div>
          </div>

          <div className="tray-status-strip">
            <div className="tray-strip-row">
              <span className="tray-strip-label">{t("tray.upstream")}</span>
              <span
                className="tray-strip-endpoint mono"
                title={summary.proxyAddress ?? t("tray.noProxy")}
              >
                {summary.proxyAddress ?? t("tray.noProxy")}
              </span>
            </div>
            <div className="tray-strip-divider" />
            <div className="tray-strip-row tray-strip-telemetry">
              {summary.running ? (
                <>
                  <span className="tray-strip-stat">
                    {t("tray.activeStreams", { count: summary.activeConnections })}
                  </span>
                  <span className="tray-speed-pair mono">
                    <span className="tray-speed-item">
                      <ArrowUpIcon size={10} />
                      <span>{formatSpeed(summary.txBytesPerSec)}</span>
                    </span>
                    <span className="tray-speed-item">
                      <ArrowDownIcon size={10} />
                      <span>{formatSpeed(summary.rxBytesPerSec)}</span>
                    </span>
                  </span>
                </>
              ) : (
                <>
                  <span className="tray-strip-stat">
                    {t("tray.enabledRules", { count: summary.enabledRules })}
                  </span>
                  <span className="tray-strip-stat tray-strip-hint">
                    {summary.needsAttention ? (
                      <>
                        <span className="dot dot-warn" />
                        <span>{t("tray.driverAttention")}</span>
                      </>
                    ) : driverReadiness === "files_ready_needs_privilege" ? (
                      <span>{t("tray.driverFilesReady")}</span>
                    ) : (
                      <span className="mono">
                        {status?.windivert_version
                          ? `WinDivert ${status.windivert_version}`
                          : t("tray.statusOff")}
                      </span>
                    )}
                  </span>
                </>
              )}
            </div>
          </div>

          {errorMsg && (
            <button
              type="button"
              className="tray-error-bar"
              onClick={() => handleOpenWindow("diagnostics")}
              title={errorMsg}
            >
              <span className="dot dot-err" />
              <span className="tray-error-text">{errorMsg}</span>
            </button>
          )}

          <ModeSwitcher
            mode={summary.runMode}
            switchingTo={switchingMode}
            status={status}
            compact
            onSelectMode={(target) => void handleSelectMode(target)}
          />
        </div>

        <div className="tray-divider" />

        <div className="tray-section">
          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-item tray-item-primary"
            onClick={() => handleOpenWindow()}
          >
            <span className="tray-item-left">
              <AppWindowIcon size={14} />
              <span>{t("tray.show")}</span>
            </span>
            <ChevronRightIcon size={13} className="tray-item-chevron" />
          </button>

          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-item"
            onClick={() => handleOpenWindow("connections")}
          >
            <span className="tray-item-left">
              <ActivityIcon size={14} />
              <span>{t("nav.connections")}</span>
            </span>
            {summary.activeConnections > 0 && (
              <span className="tray-item-meta">{summary.activeConnections}</span>
            )}
          </button>

          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-item"
            onClick={() => handleOpenWindow("applications")}
          >
            <span className="tray-item-left">
              <AppsIcon size={14} />
              <span>{t("nav.applications")}</span>
            </span>
            <span className="tray-item-meta">{summary.enabledRules}</span>
          </button>

          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-item"
            onClick={() => handleOpenWindow("proxies")}
          >
            <span className="tray-item-left">
              <RouteIcon size={14} />
              <span>{t("nav.proxies")}</span>
            </span>
            <span className="tray-item-meta">{proxiesCount}</span>
          </button>

          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-item"
            onClick={() => handleOpenWindow("diagnostics")}
          >
            <span className="tray-item-left">
              <ShieldIcon size={14} />
              <span>{t("nav.diagnostics")}</span>
            </span>
            {summary.needsAttention && <span className="dot dot-warn" />}
          </button>
        </div>

        <div className="tray-divider" />

        <div className="tray-prefs-bar">
          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-pref-btn"
            title={t("topbar.switchLang")}
            aria-label={t("topbar.switchLang")}
            onClick={toggleLocale}
          >
            <LanguageIcon size={13} />
            <span>{locale === "zh-CN" ? t("lang.zh") : t("lang.en")}</span>
          </button>

          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-pref-btn"
            title={`${t("topbar.switchTheme")} (${themeLabel})`}
            aria-label={`${t("topbar.switchTheme")} (${themeLabel})`}
            onClick={cycleMode}
          >
            {themeMode === "dark" ? (
              <MoonIcon size={13} />
            ) : themeMode === "light" ? (
              <SunIcon size={13} />
            ) : (
              <MonitorIcon size={13} />
            )}
            <span>{themeLabel}</span>
          </button>
        </div>

        <div className="tray-divider" />

        <div className="tray-section">
          <button
            type="button"
            role="menuitem"
            data-tray-focusable="true"
            className="tray-item tray-item-danger"
            onClick={handleQuit}
          >
            <span className="tray-item-left">
              <PowerIcon size={14} />
              <span>{t("tray.quit")}</span>
            </span>
          </button>
        </div>
      </div>
    </div>
  );
}
