import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowUpRightIcon,
  CopyIcon,
  DownloadIcon,
  FolderIcon,
  RefreshIcon,
  UploadIcon,
} from "../components/Icons";
import { api } from "../lib/api";
import { formatTime } from "../lib/format";
import { useI18n } from "../lib/i18n";
import {
  classifyDoctorDriverReadiness,
  classifyRuntimeError,
  deriveRunMode,
} from "../lib/mode";
import { useTheme, type ThemeMode } from "../lib/theme";
import {
  APP_VERSION,
  type AgentInfo,
  type Config,
  type DaemonStatus,
  type DoctorReport,
  type LogEntry,
  type ProxyRunMode,
  type ServiceStateSummary,
  type UpdateCheckReport,
} from "../lib/types";

interface DiagnosticsViewProps {
  status: DaemonStatus | null;
  config: Config;
  configPath: string;
  logs: LogEntry[];
  saving: boolean;
  switchingMode: ProxyRunMode | null;
  updateReport: UpdateCheckReport | null;
  checkingUpdates: boolean;
  updateError: string | null;
  onSelectMode: (target: ProxyRunMode) => void;
  onCheckUpdates: () => void;
  onSaveConfig: (next: Config, toast?: string) => Promise<void>;
  onRefreshStatus: () => Promise<void>;
  onNotify: (message: string, isError?: boolean) => void;
}

type LogLevelFilter = "ALL" | "WARN" | "ERROR";

const RELEASES_FALLBACK_URL = "https://github.com/Nexusyyan/bypath/releases";

export function DiagnosticsView({
  status,
  config,
  configPath,
  logs,
  saving,
  switchingMode,
  updateReport,
  checkingUpdates,
  updateError,
  onSelectMode,
  onCheckUpdates,
  onSaveConfig,
  onRefreshStatus,
  onNotify,
}: DiagnosticsViewProps) {
  const { locale, setLocale, t } = useI18n();
  const { mode: themeMode, setMode: setThemeMode } = useTheme();
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [runningDoctor, setRunningDoctor] = useState(false);
  const [serviceState, setServiceState] = useState<ServiceStateSummary | null>(null);
  const [serviceBusy, setServiceBusy] = useState(false);
  const [serviceError, setServiceError] = useState<string | null>(null);
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [togglingLaunchAtLogin, setTogglingLaunchAtLogin] = useState(false);
  const [windivertDir, setWindivertDir] = useState(config.general.windivert_dir);
  const [logLevel, setLogLevel] = useState<LogLevelFilter>("ALL");
  const [agentInfo, setAgentInfo] = useState<AgentInfo | null>(null);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    setWindivertDir(config.general.windivert_dir);
  }, [config.general.windivert_dir]);

  const runChecks = useCallback(async () => {
    setRunningDoctor(true);
    try {
      const [doc, svc, agent, startupEnabled] = await Promise.all([
        api.runDoctor(),
        api.getServiceStatus(),
        api.getAgentInfo(),
        api.getLaunchAtLogin().catch(() => false),
      ]);
      setDoctor(doc);
      setServiceState(svc);
      setAgentInfo(agent);
      setLaunchAtLogin(startupEnabled);
    } finally {
      setRunningDoctor(false);
    }
  }, []);

  useEffect(() => {
    void runChecks();
  }, [runChecks, status?.backend_mode, status?.engine_running, status?.service_running]);

  const handleServiceAction = async (
    action: "install" | "start" | "stop" | "uninstall"
  ) => {
    setServiceBusy(true);
    setServiceError(null);
    try {
      const res = await api.manageWindowsService(action);
      setServiceState(res);
      await onRefreshStatus();
      await runChecks();
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      const code = classifyRuntimeError(raw);
      setServiceError(
        code === "service_cancelled"
          ? t("mode.errServiceCancelled")
          : code === "elevation_cancelled"
            ? t("mode.errElevationCancelled")
            : raw
      );
    } finally {
      setServiceBusy(false);
    }
  };

  const handleToggleLaunchAtLogin = async () => {
    setTogglingLaunchAtLogin(true);
    try {
      const nextVal = await api.setLaunchAtLogin(!launchAtLogin);
      setLaunchAtLogin(nextVal);
      onNotify(
        nextVal ? t("diag.toastLaunchAtLoginOn") : t("diag.toastLaunchAtLoginOff"),
        false
      );
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), true);
    } finally {
      setTogglingLaunchAtLogin(false);
    }
  };

  const toggleGeneral = async (
    key:
      | "enable_sniffing"
      | "block_quic_when_no_udp"
      | "bypass_lan"
      | "minimize_to_tray"
      | "auto_start_engine"
      | "auto_check_updates"
  ) => {
    const currentVal =
      key === "auto_check_updates"
        ? (config.general.auto_check_updates ?? true)
        : config.general[key];
    const next: Config = {
      ...config,
      general: {
        ...config.general,
        [key]: !currentVal,
      },
    };
    await onSaveConfig(next, t("diag.toastPrefUpdated"));
  };

  const handleThemeChange = async (nextTheme: ThemeMode) => {
    setThemeMode(nextTheme);
    const next: Config = {
      ...config,
      general: {
        ...config.general,
        theme: nextTheme,
      },
    };
    await onSaveConfig(next, t("diag.toastPrefUpdated"));
  };

  const saveWindivertDir = async (e: React.FormEvent) => {
    e.preventDefault();
    const next: Config = {
      ...config,
      general: {
        ...config.general,
        windivert_dir: windivertDir.trim(),
      },
    };
    await onSaveConfig(next, t("diag.toastWdSaved"));
    await runChecks();
  };

  const handleBrowseWindivert = async () => {
    const picked = await api.pickDirectory(t("diag.wdDirTitle"));
    if (!picked) return;
    setWindivertDir(picked);
    const next: Config = {
      ...config,
      general: {
        ...config.general,
        windivert_dir: picked,
      },
    };
    await onSaveConfig(next, t("diag.toastWdSaved"));
    await runChecks();
  };

  const handleExportToml = async () => {
    try {
      const tomlText = await api.exportConfigToml();
      const blob = new Blob([tomlText], { type: "text/plain;charset=utf-8" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "bypath-config.toml";
      a.click();
      URL.revokeObjectURL(url);
      onNotify(t("diag.toastExported"), false);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), true);
    }
  };

  const handleImportTomlFile = async (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (!file) return;
    try {
      const raw = await file.text();
      await api.importConfigToml(raw);
      await onRefreshStatus();
      onNotify(t("diag.toastImported"), false);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), true);
    }
  };

  const copyText = (text: string) => {
    void navigator.clipboard.writeText(text).then(() => {
      onNotify(t("connections.copied"), false);
    });
  };

  const filteredLogs = useMemo(() => {
    return logs.filter((entry) => {
      if (logLevel === "ERROR") return entry.level === "ERROR";
      if (logLevel === "WARN") return entry.level === "WARN" || entry.level === "ERROR";
      return true;
    });
  }, [logs, logLevel]);

  const mcpSnippet = useMemo(() => {
    const cli = agentInfo?.cli_path ?? "bypath.exe";
    return JSON.stringify(
      {
        mcpServers: {
          bypath: {
            command: cli,
            args: ["mcp"],
          },
        },
      },
      null,
      2
    );
  }, [agentInfo]);

  const cliSnippet = useMemo(() => {
    const cli = agentInfo?.cli_path ?? "bypath.exe";
    return `"${cli}" ctl --json status\n"${cli}" ctl --json system-proxy\n"${cli}" ctl --json rule test --process Antigravity.exe --host aicode.googleapis.com`;
  }, [agentInfo]);

  const handleCopyDiagSummary = () => {
    const summary = JSON.stringify(
      {
        status,
        doctor,
        serviceState,
        recentLogs: filteredLogs.slice(0, 40),
      },
      null,
      2
    );
    copyText(summary);
  };

  const runMode = deriveRunMode(status);
  const effectiveServiceRunning = serviceState?.running ?? status?.service_running ?? false;
  const effectiveServiceInstalled = serviceState?.installed ?? status?.service_installed ?? false;
  const driverReadiness = classifyDoctorDriverReadiness(doctor, effectiveServiceRunning);
  const driverBlocked = driverReadiness === "blocked_by_security";
  const modeBusy = switchingMode !== null || serviceBusy;

  const bgTagLabel =
    runMode === "background"
      ? t("diag.modeBgTagActive")
      : effectiveServiceRunning
        ? t("diag.modeBgTagStandby")
        : effectiveServiceInstalled
          ? t("diag.modeBgTagStopped")
          : t("diag.modeBgTagNotInstalled");

  const fgTagLabel =
    runMode === "foreground"
      ? t("diag.modeFgTagActive")
      : status?.elevated
        ? t("diag.modeFgTagReady")
        : t("diag.modeFgTagNeedsUac");

  const contextDesc =
    runMode === "background"
      ? t("diag.contextBgActive")
      : runMode === "foreground"
        ? t("diag.contextFgActive")
        : effectiveServiceRunning
          ? t("diag.contextOffService")
          : status?.elevated
            ? t("diag.contextOffAdmin")
            : t("diag.contextOffStandard");

  const contextTag =
    runMode === "background"
      ? t("diag.tagBackground")
      : runMode === "foreground"
        ? t("diag.tagForeground")
        : t("diag.tagOff");

  const driverDotClass =
    driverReadiness === "ready"
      ? "dot-ok"
      : driverReadiness === "files_ready_needs_privilege" || driverReadiness === "unknown"
        ? "dot-idle"
        : "dot-err";

  const driverTagLabel =
    driverReadiness === "ready"
      ? doctor?.windivert_version ?? t("diag.tagReady")
      : driverReadiness === "files_ready_needs_privilege"
        ? t("diag.tagFilesReady")
        : driverReadiness === "blocked_by_security"
          ? t("diag.tagBlocked")
          : driverReadiness === "missing"
            ? t("diag.tagMissing")
            : t("diag.tagError");

  return (
    <div className="view-container">
      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.modesTitle")}</h2>
            <p className="section-desc">{t("diag.modesDesc")}</p>
          </div>
          <span className="tag tag-mono">
            <span
              className={`dot ${runMode !== "off" ? "dot-ok" : "dot-idle"}`}
              aria-hidden="true"
            />
            <span>{contextTag}</span>
          </span>
        </div>

        {serviceError && (
          <div className="notice-bar error" style={{ marginBottom: 12 }}>
            <span>{serviceError}</span>
          </div>
        )}

        <div className="mode-cards-grid">
          <div className={`mode-card ${runMode === "foreground" ? "active" : ""}`}>
            <div className="mode-card-head">
              <div className="mode-card-title-group">
                <span
                  className={`dot ${runMode === "foreground" ? "dot-ok" : "dot-idle"}`}
                  aria-hidden="true"
                />
                <span className="mode-card-title">{t("diag.modeFgTitle")}</span>
                <span className="mode-card-sub">{t("diag.modeFgSubtitle")}</span>
              </div>
              <span className="tag tag-mono">{fgTagLabel}</span>
            </div>

            <p className="mode-card-desc">{t("diag.modeFgDesc")}</p>
            <div className="mode-card-meta">
              {status?.elevated ? t("diag.modeFgMetaAdmin") : t("diag.modeFgMetaStandard")}
            </div>

            <div className="mode-card-actions">
              {runMode === "foreground" ? (
                <button
                  type="button"
                  className="btn btn-sm btn-outline"
                  disabled={modeBusy}
                  onClick={() => onSelectMode("off")}
                >
                  {t("diag.modeBtnStop")}
                </button>
              ) : (
                <button
                  type="button"
                  className="btn btn-sm btn-outline"
                  disabled={modeBusy}
                  onClick={() => onSelectMode("foreground")}
                >
                  {switchingMode === "foreground"
                    ? status?.elevated
                      ? t("mode.switchingForeground")
                      : t("mode.switchingForegroundUac")
                    : status?.elevated
                      ? t("diag.modeFgBtnStart")
                      : t("diag.modeFgBtnElevate")}
                </button>
              )}
            </div>
          </div>

          <div className={`mode-card ${runMode === "background" ? "active" : ""}`}>
            <div className="mode-card-head">
              <div className="mode-card-title-group">
                <span
                  className={`dot ${runMode === "background" ? "dot-ok" : "dot-idle"}`}
                  aria-hidden="true"
                />
                <span className="mode-card-title">{t("diag.modeBgTitle")}</span>
                <span className="mode-card-sub">{t("diag.modeBgSubtitle")}</span>
              </div>
              <span className="tag tag-mono">{bgTagLabel}</span>
            </div>

            <p className="mode-card-desc">{t("diag.modeBgDesc")}</p>
            <div className="mode-card-meta mono" title={configPath}>
              {t("diag.serviceMeta", { path: configPath })}
            </div>

            <div className="mode-card-actions">
              <div className="toolbar-group">
                {runMode === "background" ? (
                  <button
                    type="button"
                    className="btn btn-sm btn-outline"
                    disabled={modeBusy}
                    onClick={() => onSelectMode("off")}
                  >
                    {t("diag.modeBtnStop")}
                  </button>
                ) : (
                  <button
                    type="button"
                    className="btn btn-sm btn-primary"
                    disabled={modeBusy}
                    onClick={() => onSelectMode("background")}
                  >
                    {switchingMode === "background"
                      ? t("mode.switchingBackground")
                      : effectiveServiceInstalled
                        ? t("diag.modeBgBtnStart")
                        : t("diag.modeBgBtnInstall")}
                  </button>
                )}
              </div>

              {effectiveServiceInstalled && (
                <div className="toolbar-group">
                  {effectiveServiceRunning && (
                    <button
                      type="button"
                      className="btn btn-sm btn-ghost"
                      disabled={modeBusy}
                      onClick={() => void handleServiceAction("stop")}
                    >
                      {t("diag.serviceLifecycleStop")}
                    </button>
                  )}
                  <button
                    type="button"
                    className="btn btn-sm btn-danger-ghost"
                    disabled={modeBusy}
                    onClick={() => void handleServiceAction("uninstall")}
                  >
                    {t("diag.serviceLifecycleUninstall")}
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.healthTitle")}</h2>
            <p className="section-desc">{t("diag.healthDesc")}</p>
          </div>
          <button
            type="button"
            className="btn btn-sm btn-outline"
            disabled={runningDoctor}
            onClick={() => void runChecks()}
          >
            <RefreshIcon size={13} />
            <span>{runningDoctor ? t("diag.checking") : t("diag.runChecks")}</span>
          </button>
        </div>

        <div className="row-list">
          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">
                <span className={`dot ${runMode !== "off" ? "dot-ok" : "dot-idle"}`} />
                <span>{t("diag.contextTitle")}</span>
              </div>
              <div className="row-item-sub">{contextDesc}</div>
            </div>
            <span className="tag tag-mono">{contextTag}</span>
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">
                <span className={`dot ${driverDotClass}`} />
                <span>{t("diag.driverTitle")}</span>
              </div>
              <div
                className={`row-item-sub ${
                  driverReadiness === "ready" || driverReadiness === "error" ? "mono" : ""
                }`}
              >
                {driverReadiness === "ready"
                  ? t("diag.driverOk", {
                      path: doctor?.windivert_dll_path ?? "WinDivert.dll",
                      version: doctor?.windivert_version ?? "v2.2",
                    })
                  : driverReadiness === "files_ready_needs_privilege"
                    ? t("diag.driverFilesReady", {
                        path: doctor?.windivert_dll_path ?? "runtime\\windivert\\WinDivert.dll",
                      })
                    : doctor?.windivert_error ?? t("diag.driverChecking")}
              </div>
              {driverBlocked && (
                <div className="row-item-sub" style={{ marginTop: 4 }}>
                  {t("diag.driverBlockedHint")}
                </div>
              )}
            </div>
            <div className="row-item-actions">
              {driverBlocked && (
                <button
                  type="button"
                  className="btn btn-sm btn-outline"
                  onClick={() => void api.openWindowsSecurity()}
                >
                  {t("diag.openSecurity")}
                </button>
              )}
              {driverReadiness === "missing" && (
                <button
                  type="button"
                  className="btn btn-sm btn-outline"
                  onClick={() => void handleBrowseWindivert()}
                >
                  {t("setup.pickDriverDir")}
                </button>
              )}
              <span className="tag tag-mono">{driverTagLabel}</span>
            </div>
          </div>

          {(doctor?.proxies ?? []).map((p) => {
            const ok = p.report.tcp_reachable && (p.report.handshake_ok ?? true);
            return (
              <div key={p.name} className="row-item">
                <div className="row-item-main">
                  <div className="row-item-title">
                    <span className={`dot ${ok ? "dot-ok" : "dot-err"}`} />
                    <span>{t("diag.upstreamPrefix", { name: p.name })}</span>
                    <span
                      className="mono"
                      style={{ fontWeight: 450, color: "var(--ink-secondary)" }}
                    >
                      {p.report.address}
                    </span>
                  </div>
                  <div className="row-item-sub">
                    {ok
                      ? t("diag.upstreamOk", {
                          ms: p.report.latency_ms ?? 0,
                          udp: p.report.udp_associate_supported
                            ? t("diag.upstreamUdpOk")
                            : t("diag.upstreamTcpOnly"),
                          pid: p.local_listener_pid
                            ? t("diag.upstreamAntiLoop", { pid: p.local_listener_pid })
                            : "",
                        })
                      : p.report.error ?? t("diag.unreachable")}
                  </div>
                </div>
                <span className="tag tag-mono">
                  {ok ? t("diag.tagOnline") : t("diag.tagOffline")}
                </span>
              </div>
            );
          })}
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.updatesTitle")}</h2>
            <p className="section-desc">{t("diag.updatesDesc")}</p>
          </div>
          <button
            type="button"
            className="btn btn-sm btn-outline"
            disabled={checkingUpdates}
            onClick={onCheckUpdates}
          >
            <RefreshIcon size={13} />
            <span>{checkingUpdates ? t("diag.checkingUpdates") : t("diag.checkUpdates")}</span>
          </button>
        </div>

        <div className="row-list">
          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">
                <span
                  className={`dot ${
                    updateError
                      ? "dot-err"
                      : updateReport?.update_available
                        ? "dot-warn"
                        : updateReport
                          ? "dot-ok"
                          : "dot-idle"
                  }`}
                />
                <span>
                  {t("diag.versionLabel", {
                    version: updateReport?.current_version ?? APP_VERSION,
                  })}
                </span>
                {updateReport && (
                  <span className="tag tag-mono">
                    {updateReport.update_available
                      ? t("diag.updateTagNew", { tag: updateReport.latest_tag })
                      : t("diag.updateTagLatest")}
                  </span>
                )}
              </div>
              <div className="row-item-sub">
                {updateError
                  ? updateError
                  : updateReport
                    ? updateReport.update_available
                      ? updateReport.published_at
                        ? t("diag.updateFoundDated", {
                            latest: updateReport.latest_tag,
                            date: updateReport.published_at.slice(0, 10),
                            via: updateReport.checked_via,
                          })
                        : t("diag.updateFound", {
                            latest: updateReport.latest_tag,
                            via: updateReport.checked_via,
                          })
                      : t("diag.updateUpToDate", {
                          latest: updateReport.latest_tag,
                          via: updateReport.checked_via,
                        })
                    : t("diag.updateIdle")}
              </div>
            </div>

            <div className="row-item-actions">
              {updateReport?.update_available ? (
                <>
                  {updateReport.primary_asset_url && (
                    <button
                      type="button"
                      className="btn btn-sm btn-primary"
                      onClick={() =>
                        void api.openExternalUrl(updateReport.primary_asset_url!).catch((e) =>
                          onNotify(e instanceof Error ? e.message : String(e), true)
                        )
                      }
                    >
                      <DownloadIcon size={12} />
                      <span>{t("diag.downloadZip")}</span>
                    </button>
                  )}
                  <button
                    type="button"
                    className="btn btn-sm btn-outline"
                    onClick={() =>
                      void api.openExternalUrl(updateReport.release_url).catch((e) =>
                        onNotify(e instanceof Error ? e.message : String(e), true)
                      )
                    }
                  >
                    <ArrowUpRightIcon size={12} />
                    <span>{t("diag.releaseNotes")}</span>
                  </button>
                </>
              ) : (
                <button
                  type="button"
                  className="btn btn-sm btn-ghost"
                  onClick={() =>
                    void api
                      .openExternalUrl(updateReport?.release_url ?? RELEASES_FALLBACK_URL)
                      .catch((e) => onNotify(e instanceof Error ? e.message : String(e), true))
                  }
                >
                  <ArrowUpRightIcon size={12} />
                  <span>{t("diag.openReleases")}</span>
                </button>
              )}
            </div>
          </div>

          {updateReport?.update_available && updateReport.release_notes && (
            <pre className="update-notes">{updateReport.release_notes}</pre>
          )}
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.prefsTitle")}</h2>
            <p className="section-desc">{t("diag.prefsDesc")}</p>
          </div>
        </div>

        <div className="row-list">
          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.langTitle")}</div>
              <div className="row-item-sub">{t("diag.langDesc")}</div>
            </div>
            <div className="segmented" role="group" aria-label={t("diag.langTitle")}>
              <button
                type="button"
                className={`segmented-btn ${locale === "zh-CN" ? "active" : ""}`}
                onClick={() => setLocale("zh-CN")}
              >
                {t("lang.zh")}
              </button>
              <button
                type="button"
                className={`segmented-btn ${locale === "en" ? "active" : ""}`}
                onClick={() => setLocale("en")}
              >
                {t("lang.en")}
              </button>
            </div>
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.themeTitle")}</div>
              <div className="row-item-sub">{t("diag.themeDesc")}</div>
            </div>
            <div className="segmented" role="group" aria-label={t("diag.themeTitle")}>
              {(["system", "light", "dark"] as ThemeMode[]).map((m) => (
                <button
                  key={m}
                  type="button"
                  className={`segmented-btn ${themeMode === m ? "active" : ""}`}
                  onClick={() => void handleThemeChange(m)}
                >
                  {m === "system"
                    ? t("theme.system")
                    : m === "light"
                      ? t("theme.light")
                      : t("theme.dark")}
                </button>
              ))}
            </div>
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.launchAtLoginTitle")}</div>
              <div className="row-item-sub">{t("diag.launchAtLoginDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={launchAtLogin}
              aria-label={t("diag.launchAtLoginTitle")}
              className="switch"
              disabled={togglingLaunchAtLogin}
              onClick={() => void handleToggleLaunchAtLogin()}
            />
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.trayTitle")}</div>
              <div className="row-item-sub">{t("diag.trayDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={config.general.minimize_to_tray}
              className="switch"
              disabled={saving}
              onClick={() => void toggleGeneral("minimize_to_tray")}
            />
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.autostartTitle")}</div>
              <div className="row-item-sub">{t("diag.autostartDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={config.general.auto_start_engine}
              className="switch"
              disabled={saving}
              onClick={() => void toggleGeneral("auto_start_engine")}
            />
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.autoUpdateTitle")}</div>
              <div className="row-item-sub">{t("diag.autoUpdateDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={config.general.auto_check_updates ?? true}
              className="switch"
              disabled={saving}
              onClick={() => void toggleGeneral("auto_check_updates")}
            />
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.sniffTitle")}</div>
              <div className="row-item-sub">{t("diag.sniffDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={config.general.enable_sniffing}
              className="switch"
              disabled={saving}
              onClick={() => void toggleGeneral("enable_sniffing")}
            />
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.quicTitle")}</div>
              <div className="row-item-sub">{t("diag.quicDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={config.general.block_quic_when_no_udp}
              className="switch"
              disabled={saving}
              onClick={() => void toggleGeneral("block_quic_when_no_udp")}
            />
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.lanTitle")}</div>
              <div className="row-item-sub">{t("diag.lanDesc")}</div>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={config.general.bypass_lan}
              className="switch"
              disabled={saving}
              onClick={() => void toggleGeneral("bypass_lan")}
            />
          </div>

          <form className="row-item" onSubmit={(e) => void saveWindivertDir(e)}>
            <div className="row-item-main">
              <div className="row-item-title">{t("diag.wdDirTitle")}</div>
              <div className="row-item-sub">{t("diag.wdDirDesc")}</div>
            </div>
            <div className="row-item-actions">
              <input
                className="input mono"
                style={{ width: 220, height: 30 }}
                value={windivertDir}
                onChange={(e) => setWindivertDir(e.target.value)}
                aria-label={t("diag.wdDirTitle")}
              />
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                onClick={() => void handleBrowseWindivert()}
              >
                {t("diag.browseDir")}
              </button>
              <button
                type="submit"
                className="btn btn-sm btn-outline"
                disabled={saving || windivertDir.trim() === config.general.windivert_dir}
              >
                {t("diag.save")}
              </button>
            </div>
          </form>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">
                <span>{t("diag.configPortabilityTitle")}</span>
                {status?.portable_mode && (
                  <span className="tag tag-mono">{t("diag.portableTag")}</span>
                )}
              </div>
              <div className="row-item-sub">{t("diag.configPortabilityDesc")}</div>
            </div>
            <div className="row-item-actions">
              <input
                ref={fileInputRef}
                type="file"
                accept=".toml,text/plain"
                style={{ display: "none" }}
                onChange={(e) => void handleImportTomlFile(e)}
              />
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                onClick={() => void api.openPathInExplorer(configPath)}
              >
                <FolderIcon size={13} />
                <span>{t("diag.openFolder")}</span>
              </button>
              <button
                type="button"
                className="btn btn-sm btn-outline"
                onClick={() => void handleExportToml()}
              >
                <DownloadIcon size={13} />
                <span>{t("diag.exportToml")}</span>
              </button>
              <button
                type="button"
                className="btn btn-sm btn-outline"
                onClick={() => fileInputRef.current?.click()}
              >
                <UploadIcon size={13} />
                <span>{t("diag.importToml")}</span>
              </button>
            </div>
          </div>
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.logsTitle")}</h2>
            <p className="section-desc">{t("diag.logsDesc")}</p>
          </div>
          <div className="toolbar-group">
            <div className="segmented" role="group" aria-label={t("diag.logsTitle")}>
              {(["ALL", "WARN", "ERROR"] as LogLevelFilter[]).map((lvl) => (
                <button
                  key={lvl}
                  type="button"
                  className={`segmented-btn ${logLevel === lvl ? "active" : ""}`}
                  onClick={() => setLogLevel(lvl)}
                >
                  {lvl === "ALL"
                    ? t("diag.logsAll")
                    : lvl === "WARN"
                      ? t("diag.logsWarn")
                      : t("diag.logsError")}
                </button>
              ))}
            </div>
            <button
              type="button"
              className="btn btn-sm btn-outline"
              onClick={handleCopyDiagSummary}
            >
              <CopyIcon size={12} />
              <span>{t("diag.copyDiag")}</span>
            </button>
          </div>
        </div>

        <div className="row-list">
          {filteredLogs.length === 0 ? (
            <div className="empty-state" style={{ padding: "24px 16px" }}>
              <p className="empty-desc">{t("diag.logsEmpty")}</p>
            </div>
          ) : (
            <div className="log-box" role="log" aria-label={t("diag.logsTitle")}>
              {filteredLogs.slice(0, 120).map((entry) => {
                const lvlCls =
                  entry.level === "ERROR"
                    ? "log-lvl-error"
                    : entry.level === "WARN"
                      ? "log-lvl-warn"
                      : "log-lvl-info";
                return (
                  <div key={entry.id} className="log-row">
                    <span className="log-ts">{formatTime(entry.timestamp_ms)}</span>
                    <span className={lvlCls}>{entry.level}</span>
                    <span className="log-msg">{entry.message}</span>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.agentTitle")}</h2>
            <p className="section-desc">{t("diag.agentDesc")}</p>
          </div>
          <div className="toolbar-group">
            <button
              type="button"
              className="btn btn-sm btn-outline"
              onClick={() => copyText(mcpSnippet)}
            >
              <CopyIcon size={12} />
              <span>{t("diag.copyMcp")}</span>
            </button>
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              onClick={() => copyText(cliSnippet)}
            >
              <CopyIcon size={12} />
              <span>{t("diag.copyCli")}</span>
            </button>
          </div>
        </div>

        <pre className="code-snippet">{mcpSnippet}</pre>
      </section>
    </div>
  );
}
