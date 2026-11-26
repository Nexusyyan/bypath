import {
  ActivityIcon,
  AppsIcon,
  RouteIcon,
  SettingsIcon,
  ShieldIcon,
} from "./Icons";
import { ModeSwitcher } from "./ModeSwitcher";
import { useI18n } from "../lib/i18n";
import { deriveRunMode, needsDriverRecovery } from "../lib/mode";
import type { Config, DaemonStatus, NavTab, ProxyRunMode } from "../lib/types";

interface SidebarProps {
  activeTab: NavTab;
  onSelectTab: (tab: NavTab) => void;
  status: DaemonStatus | null;
  config: Config | null;
  switchingMode: ProxyRunMode | null;
  onSelectMode: (target: ProxyRunMode) => void;
}

export function Sidebar({
  activeTab,
  onSelectTab,
  status,
  config,
  switchingMode,
  onSelectMode,
}: SidebarProps) {
  const { t } = useI18n();
  const runMode = deriveRunMode(status);
  const running = runMode !== "off";
  const activeConns = status?.stats.active_connections ?? 0;
  const enabledRules = config?.rules.filter((r) => r.enabled) ?? [];
  const proxiesCount = config ? Object.keys(config.proxies).length : 0;
  const driverNeedsAttention = needsDriverRecovery(status);

  const footerLabel =
    runMode === "background"
      ? t("nav.modeBackground")
      : runMode === "foreground"
        ? t("nav.modeForeground")
        : status?.service_running
          ? t("nav.serviceStandby")
          : t("nav.serviceSettings");

  const footerDot = driverNeedsAttention
    ? "dot-warn"
    : running
      ? "dot-ok"
      : "dot-idle";

  return (
    <aside className="sidebar" aria-label={t("nav.ariaLabel")}>
      <div className="sidebar-cta-wrap">
        <ModeSwitcher
          mode={runMode}
          switchingTo={switchingMode}
          status={status}
          onSelectMode={onSelectMode}
        />
      </div>

      <nav className="sidebar-nav">
        <button
          type="button"
          className={`nav-item ${activeTab === "connections" ? "active" : ""}`}
          onClick={() => onSelectTab("connections")}
        >
          <span className="nav-item-left">
            <ActivityIcon size={15} />
            <span>{t("nav.connections")}</span>
          </span>
          {activeConns > 0 && <span className="nav-item-meta">{activeConns}</span>}
        </button>

        <button
          type="button"
          className={`nav-item ${activeTab === "applications" ? "active" : ""}`}
          onClick={() => onSelectTab("applications")}
        >
          <span className="nav-item-left">
            <AppsIcon size={15} />
            <span>{t("nav.applications")}</span>
          </span>
          <span className="nav-item-meta">{enabledRules.length}</span>
        </button>

        <button
          type="button"
          className={`nav-item ${activeTab === "proxies" ? "active" : ""}`}
          onClick={() => onSelectTab("proxies")}
        >
          <span className="nav-item-left">
            <RouteIcon size={15} />
            <span>{t("nav.proxies")}</span>
          </span>
          <span className="nav-item-meta">{proxiesCount}</span>
        </button>

        <button
          type="button"
          className={`nav-item ${activeTab === "diagnostics" ? "active" : ""}`}
          onClick={() => onSelectTab("diagnostics")}
        >
          <span className="nav-item-left">
            <ShieldIcon size={15} />
            <span>{t("nav.diagnostics")}</span>
          </span>
          {driverNeedsAttention && (
            <span className="dot dot-warn" title={t("nav.needsAttention")} />
          )}
        </button>
      </nav>

      <div className="sidebar-section">
        <div className="sidebar-section-label">{t("nav.activeRules")}</div>
        <div className="sidebar-rule-list">
          {enabledRules.length === 0 ? (
            <button
              type="button"
              className="sidebar-rule-item"
              onClick={() => onSelectTab("applications")}
            >
              <span className="sidebar-rule-name">{t("nav.noEnabledRules")}</span>
            </button>
          ) : (
            enabledRules.slice(0, 8).map((rule, idx) => (
              <button
                key={`${rule.name}-${idx}`}
                type="button"
                className="sidebar-rule-item"
                onClick={() => onSelectTab("applications")}
                title={rule.name}
              >
                <span className="sidebar-rule-name">
                  {rule.name.replace(/^Preset:\s*/i, "")}
                </span>
                <span
                  className={`dot ${running ? "dot-ok" : "dot-idle"}`}
                  aria-hidden="true"
                />
              </button>
            ))
          )}
        </div>
      </div>

      <div className="sidebar-footer">
        <button
          type="button"
          className="sidebar-footer-btn"
          onClick={() => onSelectTab("diagnostics")}
        >
          <span className="nav-item-left">
            <SettingsIcon size={14} />
            <span>{footerLabel}</span>
          </span>
          <span className={`dot ${footerDot}`} aria-hidden="true" />
        </button>
      </div>
    </aside>
  );
}
