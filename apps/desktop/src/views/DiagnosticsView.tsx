import { useEffect, useState } from "react";
import { RefreshIcon } from "../components/Icons";
import { api } from "../lib/api";
import { useI18n } from "../lib/i18n";
import type { Config, DaemonStatus, DoctorReport, ServiceStateSummary } from "../lib/types";

interface DiagnosticsViewProps {
  status: DaemonStatus | null;
  config: Config;
  configPath: string;
  saving: boolean;
  onSaveConfig: (next: Config, toast?: string) => Promise<void>;
  onRefreshStatus: () => Promise<void>;
}

export function DiagnosticsView({
  status,
  config,
  configPath,
  saving,
  onSaveConfig,
  onRefreshStatus,
}: DiagnosticsViewProps) {
  const { locale, setLocale, t } = useI18n();
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [runningDoctor, setRunningDoctor] = useState(false);
  const [serviceState, setServiceState] = useState<ServiceStateSummary | null>(null);
  const [serviceBusy, setServiceBusy] = useState(false);
  const [serviceError, setServiceError] = useState<string | null>(null);
  const [windivertDir, setWindivertDir] = useState(config.general.windivert_dir);

  useEffect(() => {
    setWindivertDir(config.general.windivert_dir);
  }, [config.general.windivert_dir]);

  const runChecks = async () => {
    setRunningDoctor(true);
    try {
      const [doc, svc] = await Promise.all([api.runDoctor(), api.getServiceStatus()]);
      setDoctor(doc);
      setServiceState(svc);
    } finally {
      setRunningDoctor(false);
    }
  };

  useEffect(() => {
    void runChecks();
  }, []);

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
      setServiceError(err instanceof Error ? err.message : String(err));
    } finally {
      setServiceBusy(false);
    }
  };

  const toggleGeneral = async (
    key: "enable_sniffing" | "block_quic_when_no_udp" | "bypass_lan"
  ) => {
    const next: Config = {
      ...config,
      general: {
        ...config.general,
        [key]: !config.general[key],
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

  const formatServiceBadge = (state: string | undefined): string => {
    switch (state) {
      case "running":
        return t("diag.svcStateRunning");
      case "stopped":
        return t("diag.svcStateStopped");
      case "start_pending":
        return t("diag.svcStateStartPending");
      case "stop_pending":
        return t("diag.svcStateStopPending");
      default:
        return t("diag.svcStateNotInstalled");
    }
  };

  return (
    <div className="view-container">
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
                <span className={`dot ${doctor?.elevated ? "dot-ok" : "dot-warn"}`} />
                <span>{t("diag.contextTitle")}</span>
              </div>
              <div className="row-item-sub">
                {doctor?.elevated
                  ? status?.backend_mode === "service"
                    ? t("diag.contextService")
                    : t("diag.contextAdmin")
                  : t("diag.contextStandard")}
              </div>
            </div>
            <span className="tag tag-mono">
              {status?.backend_mode === "service"
                ? t("diag.tagIpcService")
                : t("diag.tagLocal")}
            </span>
          </div>

          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">
                <span
                  className={`dot ${
                    doctor?.windivert_driver_opened
                      ? "dot-ok"
                      : doctor?.windivert_dll_loaded
                        ? "dot-warn"
                        : "dot-err"
                  }`}
                />
                <span>{t("diag.driverTitle")}</span>
              </div>
              <div className="row-item-sub mono">
                {doctor?.windivert_driver_opened
                  ? t("diag.driverOk", {
                      path: doctor.windivert_dll_path ?? "WinDivert.dll",
                      version: doctor.windivert_version ?? "v2.2",
                    })
                  : doctor?.windivert_error ?? t("diag.driverChecking")}
              </div>
            </div>
            <span className="tag tag-mono">
              {doctor?.windivert_driver_opened
                ? doctor.windivert_version ?? t("diag.tagReady")
                : doctor?.windivert_dll_loaded
                  ? t("diag.tagNeedsAdmin")
                  : t("diag.tagMissing")}
            </span>
          </div>

          {(doctor?.proxies ?? []).map((p) => (
            <div key={p.name} className="row-item">
              <div className="row-item-main">
                <div className="row-item-title">
                  <span
                    className={`dot ${p.report.tcp_reachable ? "dot-ok" : "dot-err"}`}
                  />
                  <span>{t("diag.upstreamPrefix", { name: p.name })}</span>
                  <span
                    className="mono"
                    style={{ fontWeight: 450, color: "var(--ink-secondary)" }}
                  >
                    {p.report.address}
                  </span>
                </div>
                <div className="row-item-sub">
                  {p.report.tcp_reachable
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
                {p.report.tcp_reachable ? t("diag.tagOnline") : t("diag.tagOffline")}
              </span>
            </div>
          ))}
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("diag.serviceTitle")}</h2>
            <p className="section-desc">{t("diag.serviceDesc")}</p>
          </div>
        </div>

        {serviceError && (
          <div className="notice-bar error" style={{ marginBottom: 12 }}>
            <span>{serviceError}</span>
          </div>
        )}

        <div className="row-list">
          <div className="row-item">
            <div className="row-item-main">
              <div className="row-item-title">
                <span
                  className={`dot ${
                    serviceState?.running
                      ? "dot-ok"
                      : serviceState?.installed
                        ? "dot-warn"
                        : "dot-idle"
                  }`}
                />
                <span>BypathService</span>
                <span className="tag tag-mono">
                  {formatServiceBadge(serviceState?.state)}
                </span>
              </div>
              <div className="row-item-sub mono">
                {t("diag.serviceMeta", { path: configPath })}
              </div>
            </div>

            <div className="row-item-actions">
              {!serviceState?.installed ? (
                <button
                  type="button"
                  className="btn btn-sm btn-primary"
                  disabled={serviceBusy}
                  onClick={() => void handleServiceAction("install")}
                >
                  {serviceBusy ? t("diag.serviceInstalling") : t("diag.serviceInstall")}
                </button>
              ) : (
                <>
                  {serviceState.running ? (
                    <button
                      type="button"
                      className="btn btn-sm btn-outline"
                      disabled={serviceBusy}
                      onClick={() => void handleServiceAction("stop")}
                    >
                      {t("diag.serviceStop")}
                    </button>
                  ) : (
                    <button
                      type="button"
                      className="btn btn-sm btn-primary"
                      disabled={serviceBusy}
                      onClick={() => void handleServiceAction("start")}
                    >
                      {t("diag.serviceStart")}
                    </button>
                  )}
                  <button
                    type="button"
                    className="btn btn-sm btn-danger-ghost"
                    disabled={serviceBusy}
                    onClick={() => void handleServiceAction("uninstall")}
                  >
                    {t("diag.serviceUninstall")}
                  </button>
                </>
              )}
            </div>
          </div>
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
                style={{ width: 240, height: 30 }}
                value={windivertDir}
                onChange={(e) => setWindivertDir(e.target.value)}
                aria-label={t("diag.wdDirTitle")}
              />
              <button
                type="submit"
                className="btn btn-sm btn-outline"
                disabled={saving || windivertDir.trim() === config.general.windivert_dir}
              >
                {t("diag.save")}
              </button>
            </div>
          </form>
        </div>
      </section>
    </div>
  );
}
