import { useCallback, useEffect, useRef, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { Titlebar } from "./components/Titlebar";
import { api } from "./lib/api";
import { useI18n, type MessageKey } from "./lib/i18n";
import type {
  Config,
  ConnectionRecord,
  DaemonStatus,
  EngineEvent,
  NavTab,
  PresetInfo,
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

export function App() {
  const { t } = useI18n();
  const [activeTab, setActiveTab] = useState<NavTab>(initialTab);
  const [status, setStatus] = useState<DaemonStatus | null>(null);
  const [config, setConfig] = useState<Config | null>(null);
  const [configPath, setConfigPath] = useState("");
  const [presets, setPresets] = useState<PresetInfo[]>([]);
  const [connections, setConnections] = useState<ConnectionRecord[]>([]);
  const [paused, setPaused] = useState(false);
  const [togglingEngine, setTogglingEngine] = useState(false);
  const [saving, setSaving] = useState(false);
  const [notice, setNotice] = useState<{ text: string; isError: boolean } | null>(null);

  const pausedRef = useRef(paused);
  pausedRef.current = paused;

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
      const [st, cfgSnap, conns, presetList] = await Promise.all([
        api.getStatus(),
        api.getConfig(),
        api.getConnections(),
        api.listPresets(),
      ]);
      setStatus(st);
      setConfig(cfgSnap.config);
      setConfigPath(cfgSnap.path);
      setConnections(conns);
      setPresets(presetList);
    } catch (err) {
      flashNotice(err instanceof Error ? err.message : String(err), true);
    }
  }, [flashNotice]);

  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void api
      .subscribeEvents(
        (evt: EngineEvent) => {
          switch (evt.type) {
            case "stats_tick":
              setStatus((prev) => (prev ? { ...prev, stats: evt.stats } : prev));
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
                flashNotice(evt.error, true);
              }
              break;
            case "config_reloaded":
              void refreshAll();
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
  }, [flashNotice, refreshAll]);

  const handleToggleEngine = async () => {
    setTogglingEngine(true);
    setNotice(null);
    try {
      if (status?.engine_running) {
        await api.stopEngine();
        flashNotice(t("engine.toastStopped"));
      } else {
        await api.startEngine();
        flashNotice(t("engine.toastStarted"));
      }
      await refreshAll();
    } catch (err) {
      flashNotice(err instanceof Error ? err.message : String(err), true);
    } finally {
      setTogglingEngine(false);
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
      flashNotice(err instanceof Error ? err.message : String(err), true);
    } finally {
      setSaving(false);
    }
  };

  const handleClearClosed = async () => {
    try {
      await api.clearConnections();
      setConnections((prev) => prev.filter((c) => c.state === "active"));
    } catch (err) {
      flashNotice(err instanceof Error ? err.message : String(err), true);
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
      const already = target.domains.some((d) => d.toLowerCase() === pattern);
      if (!already) {
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

  const firstProxyAddress = config
    ? Object.values(config.proxies)[0]?.address ?? null
    : null;

  return (
    <div className="app-frame">
      <Titlebar
        sectionLabel={t(TAB_LABEL_KEYS[activeTab])}
        status={status}
        firstProxyAddress={firstProxyAddress}
        onRefresh={() => void refreshAll()}
      />

      <div className="app-body">
        <Sidebar
          activeTab={activeTab}
          onSelectTab={setActiveTab}
          status={status}
          config={config}
          togglingEngine={togglingEngine}
          onToggleEngine={() => void handleToggleEngine()}
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

            {activeTab === "connections" && (
              <ConnectionsView
                status={status}
                config={config}
                connections={connections}
                paused={paused}
                saving={saving}
                onTogglePause={() => setPaused((v) => !v)}
                onClearClosed={() => void handleClearClosed()}
                onStartEngine={() => void handleToggleEngine()}
                onOpenDiagnostics={() => setActiveTab("diagnostics")}
                onBlockDomain={(pattern) => void handleBlockDomain(pattern)}
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
              />
            )}

            {activeTab === "diagnostics" && config && (
              <DiagnosticsView
                status={status}
                config={config}
                configPath={configPath}
                saving={saving}
                onSaveConfig={handleSaveConfig}
                onRefreshStatus={refreshAll}
              />
            )}
          </div>
        </main>
      </div>
    </div>
  );
}
