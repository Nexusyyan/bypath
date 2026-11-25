import { useState } from "react";
import { EditIcon, GlobeIcon, PlusIcon, RefreshIcon, TrashIcon } from "../components/Icons";
import { Select } from "../components/Select";
import { api } from "../lib/api";
import { useI18n } from "../lib/i18n";
import type { Config, NamedProxyProbe, ProxyConfig, ProxyKind } from "../lib/types";

interface ProxiesViewProps {
  config: Config;
  saving: boolean;
  onSaveConfig: (next: Config, toast?: string) => Promise<void>;
  onNotify: (message: string, isError?: boolean) => void;
}

export function ProxiesView({ config, saving, onSaveConfig, onNotify }: ProxiesViewProps) {
  const { t } = useI18n();
  const [showForm, setShowForm] = useState(false);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [kind, setKind] = useState<ProxyKind>("socks5");
  const [address, setAddress] = useState("127.0.0.1:10808");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [udp, setUdp] = useState(true);

  const [probes, setProbes] = useState<Record<string, NamedProxyProbe>>({});
  const [probingKey, setProbingKey] = useState<string | null>(null);
  const [readingSystem, setReadingSystem] = useState(false);

  const entries = Object.entries(config.proxies).sort(([a], [b]) => a.localeCompare(b));

  const runProbe = async (key: string, proxy: ProxyConfig) => {
    setProbingKey(key);
    try {
      const res = await api.probeProxy(proxy);
      setProbes((prev) => ({ ...prev, [key]: res }));
    } finally {
      setProbingKey(null);
    }
  };

  const openAddForm = () => {
    if (showForm && editingKey === null) {
      setShowForm(false);
      return;
    }
    setEditingKey(null);
    setName("");
    setKind("socks5");
    setAddress("127.0.0.1:10808");
    setUsername("");
    setPassword("");
    setUdp(true);
    setShowForm(true);
  };

  const openEditForm = (key: string, proxy: ProxyConfig) => {
    setEditingKey(key);
    setName(key);
    setKind(proxy.type);
    setAddress(proxy.address);
    setUsername(proxy.username ?? "");
    setPassword(proxy.password ?? "");
    setUdp(proxy.udp);
    setShowForm(true);
  };

  const handleReadSystemProxy = async () => {
    setReadingSystem(true);
    try {
      const info = await api.getSystemProxy();
      if (!info.enabled || !info.suggested_address) {
        onNotify(t("proxies.systemNone"), false);
        return;
      }
      const firstExistingKey = entries[0]?.[0] ?? null;
      const existingProxy = firstExistingKey ? config.proxies[firstExistingKey] : null;
      if (!showForm) {
        if (firstExistingKey && existingProxy) {
          setEditingKey(firstExistingKey);
          setName(firstExistingKey);
          setUsername(existingProxy.username ?? "");
          setPassword(existingProxy.password ?? "");
        } else {
          setEditingKey(null);
          setName("system_proxy");
          setUsername("");
          setPassword("");
        }
        setShowForm(true);
      }
      const keepExistingSocks =
        existingProxy &&
        existingProxy.address === info.suggested_address &&
        existingProxy.type === "socks5" &&
        !info.socks_address;
      const nextKind: ProxyKind = keepExistingSocks
        ? "socks5"
        : (info.suggested_kind ?? "http");
      setKind(nextKind);
      setAddress(info.suggested_address);
      setUdp(keepExistingSocks ? existingProxy.udp : nextKind === "socks5");
      onNotify(t("proxies.systemFound", { address: info.suggested_address }), false);
    } catch (err) {
      onNotify(err instanceof Error ? err.message : String(err), true);
    } finally {
      setReadingSystem(false);
    }
  };

  const handleSaveProxy = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim().replace(/\s+/g, "_");
    if (!trimmed || !address.trim()) return;

    const nextProxy: ProxyConfig = {
      type: kind,
      address: address.trim(),
      username: username.trim() ? username.trim() : null,
      password: password.trim() ? password.trim() : null,
      udp: kind === "socks5" ? udp : false,
    };

    const nextProxies: Record<string, ProxyConfig> = { ...config.proxies };
    let nextRules = config.rules;

    if (editingKey && editingKey !== trimmed) {
      delete nextProxies[editingKey];
      nextRules = config.rules.map((r) =>
        r.proxy === editingKey ? { ...r, proxy: trimmed } : r
      );
    }
    nextProxies[trimmed] = nextProxy;

    const nextConfig: Config = {
      ...config,
      proxies: nextProxies,
      rules: nextRules,
    };
    await onSaveConfig(nextConfig, t("proxies.toastSaved", { name: trimmed }));
    setEditingKey(null);
    setName("");
    setUsername("");
    setPassword("");
    setShowForm(false);
    void runProbe(trimmed, nextProxy);
  };

  const toggleUdp = async (key: string, current: ProxyConfig) => {
    if (current.type !== "socks5") return;
    const nextConfig: Config = {
      ...config,
      proxies: {
        ...config.proxies,
        [key]: { ...current, udp: !current.udp },
      },
    };
    await onSaveConfig(nextConfig, t("proxies.toastUdpUpdated", { name: key }));
  };

  const removeProxy = async (key: string) => {
    const referenced = config.rules.some(
      (r) => r.enabled && r.action === "proxy" && r.proxy === key
    );
    if (referenced) return;
    const nextProxies = { ...config.proxies };
    delete nextProxies[key];
    await onSaveConfig(
      { ...config, proxies: nextProxies },
      t("proxies.toastRemoved", { name: key })
    );
  };

  return (
    <div className="view-container">
      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("proxies.title")}</h2>
            <p className="section-desc">{t("proxies.desc")}</p>
          </div>
          <div className="toolbar-group">
            <button
              type="button"
              className="btn btn-sm btn-outline"
              disabled={readingSystem}
              onClick={() => void handleReadSystemProxy()}
            >
              <GlobeIcon size={13} />
              <span>
                {readingSystem ? t("proxies.readingSystem") : t("proxies.readSystem")}
              </span>
            </button>
            <button
              type="button"
              className={`btn btn-sm ${showForm && editingKey === null ? "btn-secondary" : "btn-primary"}`}
              onClick={openAddForm}
            >
              <PlusIcon size={13} />
              <span>{t("proxies.add")}</span>
            </button>
          </div>
        </div>

        {showForm && (
          <form
            className="composer-panel"
            onSubmit={(e) => void handleSaveProxy(e)}
            style={{ marginBottom: 14 }}
          >
            <div className="section-title" style={{ fontSize: 14 }}>
              {editingKey ? t("proxies.editTitle") : t("proxies.addTitle")}
            </div>

            <div className="form-grid">
              <div className="form-field">
                <label className="form-label" htmlFor="proxy-name">
                  {t("proxies.profileName")}
                </label>
                <input
                  id="proxy-name"
                  className="input mono"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={t("proxies.profilePlaceholder")}
                  required
                />
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="proxy-kind">
                  {t("proxies.protocol")}
                </label>
                <Select<ProxyKind>
                  id="proxy-kind"
                  value={kind}
                  ariaLabel={t("proxies.protocol")}
                  onChange={(val) => setKind(val)}
                  options={[
                    { value: "socks5", label: t("proxies.protoSocks5") },
                    { value: "http", label: t("proxies.protoHttp") },
                  ]}
                />
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="proxy-address">
                  {t("proxies.endpoint")}
                </label>
                <input
                  id="proxy-address"
                  className="input mono"
                  value={address}
                  onChange={(e) => setAddress(e.target.value)}
                  placeholder="127.0.0.1:7897"
                  required
                />
              </div>

              <div className="form-field" style={{ justifyContent: "center" }}>
                <span className="form-label">{t("proxies.udpLabel")}</span>
                <div style={{ display: "flex", alignItems: "center", gap: 10, height: 34 }}>
                  <button
                    type="button"
                    role="switch"
                    aria-checked={kind === "socks5" && udp}
                    disabled={kind !== "socks5"}
                    className="switch"
                    onClick={() => setUdp((v) => !v)}
                  />
                  <span className="form-hint">
                    {kind === "socks5" ? t("proxies.udpHintOn") : t("proxies.udpHintOff")}
                  </span>
                </div>
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="proxy-user">
                  {t("proxies.username")}
                </label>
                <input
                  id="proxy-user"
                  className="input"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  placeholder={t("proxies.usernamePlaceholder")}
                />
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="proxy-pass">
                  {t("proxies.password")}
                </label>
                <input
                  id="proxy-pass"
                  className="input"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder={t("proxies.passwordPlaceholder")}
                />
              </div>
            </div>

            <div className="form-actions">
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => {
                  setEditingKey(null);
                  setShowForm(false);
                }}
              >
                {t("apps.cancel")}
              </button>
              <button
                type="submit"
                className="btn btn-primary"
                disabled={saving || !name.trim() || !address.trim()}
              >
                {t("proxies.save")}
              </button>
            </div>
          </form>
        )}

        <div className="row-list">
          {entries.length === 0 ? (
            <div className="empty-state">
              <h3 className="empty-title">{t("proxies.emptyTitle")}</h3>
              <p className="empty-desc">{t("proxies.emptyDesc")}</p>
            </div>
          ) : (
            entries.map(([key, proxy]) => {
              const probe = probes[key];
              const isReferenced = config.rules.some(
                (r) => r.enabled && r.action === "proxy" && r.proxy === key
              );
              const probeOnline = Boolean(
                probe && probe.report.tcp_reachable && (probe.report.handshake_ok ?? true)
              );
              return (
                <div key={key} className="row-item">
                  <div className="row-item-main">
                    <div className="row-item-title">
                      <span>{key}</span>
                      <span className="tag tag-mono">{proxy.type.toUpperCase()}</span>
                      <span
                        className="mono"
                        style={{ fontWeight: 450, color: "var(--ink-medium)" }}
                      >
                        {proxy.address}
                      </span>
                      {proxy.username && <span className="tag">{t("proxies.authTag")}</span>}
                    </div>

                    <div className="row-item-sub">
                      {probe ? (
                        probeOnline ? (
                          <span style={{ display: "inline-flex", alignItems: "center", gap: 7 }}>
                            <span className="dot dot-ok" />
                            <span>
                              {t("proxies.probeOk", {
                                ms: probe.report.latency_ms ?? 0,
                                udp: probe.report.udp_associate_supported
                                  ? t("proxies.probeUdpOk")
                                  : t("proxies.probeTcpOnly"),
                                pid: probe.local_listener_pid
                                  ? t("proxies.probeAntiLoop", {
                                      pid: probe.local_listener_pid,
                                    })
                                  : "",
                              })}
                            </span>
                          </span>
                        ) : (
                          <span
                            style={{
                              display: "inline-flex",
                              alignItems: "center",
                              gap: 7,
                              color: "var(--status-err)",
                            }}
                          >
                            <span className="dot dot-err" />
                            <span>
                              {probe.report.tcp_reachable && probe.report.handshake_ok === false
                                ? t("proxies.probeHandshakeFail", {
                                    error: probe.report.error ?? t("proxies.probeFailed"),
                                  })
                                : probe.report.error ?? t("proxies.probeFailed")}
                            </span>
                          </span>
                        )
                      ) : (
                        <span>
                          {proxy.type === "socks5"
                            ? proxy.udp
                              ? t("proxies.idleSocksUdp")
                              : t("proxies.idleSocksTcp")
                            : t("proxies.idleHttp")}
                        </span>
                      )}
                    </div>
                  </div>

                  <div className="row-item-actions">
                    {proxy.type === "socks5" && (
                      <div
                        style={{
                          display: "inline-flex",
                          alignItems: "center",
                          gap: 6,
                          marginRight: 4,
                        }}
                      >
                        <span className="cell-secondary">UDP</span>
                        <button
                          type="button"
                          role="switch"
                          aria-checked={proxy.udp}
                          aria-label={key}
                          className="switch"
                          disabled={saving}
                          onClick={() => void toggleUdp(key, proxy)}
                        />
                      </div>
                    )}

                    <button
                      type="button"
                      className="btn btn-sm btn-outline"
                      disabled={saving}
                      onClick={() => openEditForm(key, proxy)}
                    >
                      <EditIcon size={12} />
                      <span>{t("proxies.edit")}</span>
                    </button>

                    <button
                      type="button"
                      className="btn btn-sm btn-outline"
                      disabled={probingKey === key}
                      onClick={() => void runProbe(key, proxy)}
                    >
                      <RefreshIcon size={12} />
                      <span>{probingKey === key ? t("proxies.testing") : t("proxies.test")}</span>
                    </button>

                    <button
                      type="button"
                      className="icon-btn btn-danger-ghost"
                      disabled={saving || isReferenced}
                      title={isReferenced ? t("proxies.inUse") : t("proxies.delete")}
                      aria-label={t("proxies.delete")}
                      onClick={() => void removeProxy(key)}
                    >
                      <TrashIcon size={14} />
                    </button>
                  </div>
                </div>
              );
            })
          )}
        </div>
      </section>
    </div>
  );
}
