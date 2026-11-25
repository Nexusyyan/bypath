import {
  ActivityIcon,
  AppsIcon,
  PlayIcon,
  RouteIcon,
  SettingsIcon,
  ShieldIcon,
  StopIcon,
} from "./Icons";
import { useI18n } from "../lib/i18n";
import type { Config, DaemonStatus, NavTab } from "../lib/types";

interface SidebarProps {
  activeTab: NavTab;
  onSelectTab: (tab: NavTab) => void;
  status: DaemonStatus | null;
  config: Config | null;
  togglingEngine: boolean;
  onToggleEngine: () => void;
}

export function Sidebar({
  activeTab,
  onSelectTab,
  status,
  config,
  togglingEngine,
  onToggleEngine,
}: SidebarProps) {
  const { t } = useI18n();
  const running = status?.engine_running ?? false;
  const activeConns = status?.stats.active_connections ?? 0;
  const enabledRules = config?.rules.filter((r) => r.enabled) ?? [];
  const proxiesCount = config ? Object.keys(config.proxies).length : 0;
  const needsAttention = status ? !status.windivert_ready : false;

  return (
    <aside className="sidebar" aria-label={t("nav.ariaLabel")}>
      <div className="sidebar-cta-wrap">
        <button
          type="button"
          className={`engine-cta ${running ? "stop" : "start"}`}
          disabled={togglingEngine}
          onClick={onToggleEngine}
        >
          {running ? (
            <>
              <StopIcon size={11} />
              <span>{togglingEngine ? t("engine.stopping") : t("engine.stop")}</span>
            </>
          ) : (
            <>
              <PlayIcon size={12} />
              <span>{togglingEngine ? t("engine.starting") : t("engine.start")}</span>
            </>
          )}
        </button>
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
          {needsAttention && (
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
            <span>
              {status?.service_running
                ? t("nav.serviceActive")
                : status?.elevated
                  ? t("nav.adminLocal")
                  : t("nav.serviceSettings")}
            </span>
          </span>
          <span
            className={`dot ${status?.windivert_ready ? "dot-ok" : "dot-warn"}`}
            aria-hidden="true"
          />
        </button>
      </div>
    </aside>
  );
}
