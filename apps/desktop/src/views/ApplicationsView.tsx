import { useEffect, useMemo, useState } from "react";
import {
  BanIcon,
  CheckIcon,
  CloseIcon,
  GlobeIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  TerminalIcon,
  TrashIcon,
} from "../components/Icons";
import { api } from "../lib/api";
import { useI18n, type MessageKey } from "../lib/i18n";
import type { Config, PresetInfo, ProcessEntry, RuleAction, RuleConfig } from "../lib/types";

interface ApplicationsViewProps {
  config: Config;
  presets: PresetInfo[];
  saving: boolean;
  onSaveConfig: (next: Config, toast?: string) => Promise<void>;
}

const PRESET_I18N_KEYS: Record<string, { category: MessageKey; desc: MessageKey }> = {
  antigravity: {
    category: "preset.antigravity.category",
    desc: "preset.antigravity.desc",
  },
  cursor: {
    category: "preset.cursor.category",
    desc: "preset.cursor.desc",
  },
  vscode: {
    category: "preset.vscode.category",
    desc: "preset.vscode.desc",
  },
  git: {
    category: "preset.git.category",
    desc: "preset.git.desc",
  },
  discord: {
    category: "preset.discord.category",
    desc: "preset.discord.desc",
  },
  telegram: {
    category: "preset.telegram.category",
    desc: "preset.telegram.desc",
  },
  steam: {
    category: "preset.steam.category",
    desc: "preset.steam.desc",
  },
};

function presetRuleName(preset: PresetInfo): string {
  return `Preset: ${preset.name}`;
}

function parseListInput(raw: string): string[] {
  return raw
    .split(/[,\n;\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export function ApplicationsView({
  config,
  presets,
  saving,
  onSaveConfig,
}: ApplicationsViewProps) {
  const { t } = useI18n();
  const [showComposer, setShowComposer] = useState(false);
  const [showPicker, setShowPicker] = useState(false);

  const proxyNames = Object.keys(config.proxies);
  const defaultProxy = proxyNames[0] ?? "";

  const [quickDomainInput, setQuickDomainInput] = useState("");
  const [quickDomainProc, setQuickDomainProc] = useState("");

  const [ruleName, setRuleName] = useState("");
  const [processInput, setProcessInput] = useState("");
  const [pathInput, setPathInput] = useState("");
  const [domainsInput, setDomainsInput] = useState("");
  const [includeChildren, setIncludeChildren] = useState(true);
  const [action, setAction] = useState<RuleAction>("proxy");
  const [selectedProxy, setSelectedProxy] = useState(defaultProxy);

  const [processes, setProcesses] = useState<ProcessEntry[]>([]);
  const [procFilter, setProcFilter] = useState("");
  const [loadingProcs, setLoadingProcs] = useState(false);

  useEffect(() => {
    if (!selectedProxy && defaultProxy) {
      setSelectedProxy(defaultProxy);
    }
  }, [defaultProxy, selectedProxy]);

  const loadProcesses = async () => {
    setLoadingProcs(true);
    try {
      const list = await api.listProcesses();
      setProcesses(list);
    } finally {
      setLoadingProcs(false);
    }
  };

  useEffect(() => {
    if (showPicker) {
      void loadProcesses();
    }
  }, [showPicker]);

  const blockedDomainEntries = useMemo(() => {
    const entries: Array<{
      ruleIndex: number;
      domain: string;
      scope: string | null;
      enabled: boolean;
    }> = [];
    config.rules.forEach((rule, ruleIndex) => {
      if (rule.action !== "block" || !rule.domains || rule.domains.length === 0) return;
      const scope =
        rule.process_names.length > 0
          ? rule.process_names.join(", ")
          : rule.path_contains.length > 0
            ? rule.path_contains.join(", ")
            : null;
      for (const domain of rule.domains) {
        entries.push({ ruleIndex, domain, scope, enabled: rule.enabled });
      }
    });
    return entries;
  }, [config.rules]);

  const filteredProcesses = useMemo(() => {
    const q = procFilter.trim().toLowerCase();
    const seen = new Set<string>();
    return processes
      .filter((p) => {
        if (p.pid <= 4) return false;
        if (!q) return true;
        return (
          p.name.toLowerCase().includes(q) ||
          (p.exe_path ?? "").toLowerCase().includes(q) ||
          String(p.pid).includes(q)
        );
      })
      .filter((p) => {
        if (q) return true;
        const key = `${p.name.toLowerCase()}|${(p.exe_path ?? "").toLowerCase()}`;
        if (seen.has(key)) return false;
        seen.add(key);
        return true;
      })
      .slice(0, 80);
  }, [processes, procFilter]);

  const isPresetEnabled = (preset: PresetInfo): boolean => {
    const targetName = presetRuleName(preset).toLowerCase();
    return config.rules.some(
      (r) =>
        r.enabled &&
        (r.name.toLowerCase() === targetName ||
          r.name.toLowerCase().startsWith(targetName) ||
          preset.process_names.some((pn) =>
            r.process_names.some((rn) => rn.toLowerCase() === pn.toLowerCase())
          ))
    );
  };

  const togglePreset = async (preset: PresetInfo) => {
    const targetName = presetRuleName(preset);
    const nextRules = [...config.rules];
    const existingIdx = nextRules.findIndex(
      (r) =>
        r.name.toLowerCase() === targetName.toLowerCase() ||
        r.name.toLowerCase().startsWith(targetName.toLowerCase())
    );

    if (existingIdx >= 0) {
      nextRules[existingIdx] = {
        ...nextRules[existingIdx],
        enabled: !nextRules[existingIdx].enabled,
      };
    } else {
      if (!defaultProxy) return;
      nextRules.unshift({
        name: targetName,
        enabled: true,
        process_names: [...preset.process_names],
        path_prefixes: [],
        path_contains: [...preset.path_contains],
        include_children: preset.include_children,
        dst_cidrs: [],
        dst_ports: [],
        protocols: [],
        domains: [],
        action: "proxy",
        proxy: defaultProxy,
      });
    }

    await onSaveConfig(
      { ...config, rules: nextRules },
      t("apps.toastPresetUpdated", { name: preset.name })
    );
  };

  const addQuickBlockedDomains = async (e: React.FormEvent) => {
    e.preventDefault();
    const patterns = parseListInput(quickDomainInput).map((d) =>
      d.replace(/\.+$/, "").toLowerCase()
    );
    if (patterns.length === 0) return;

    const procNames = quickDomainProc
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);

    const nextRules = config.rules.map((r) => ({
      ...r,
      domains: [...(r.domains ?? [])],
    }));

    if (procNames.length === 0) {
      const idx = nextRules.findIndex(
        (r) =>
          r.action === "block" &&
          r.process_names.length === 0 &&
          r.path_prefixes.length === 0 &&
          r.path_contains.length === 0
      );
      if (idx >= 0) {
        for (const p of patterns) {
          if (!nextRules[idx].domains.some((d) => d.toLowerCase() === p)) {
            nextRules[idx].domains.push(p);
          }
        }
        nextRules[idx].enabled = true;
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
          domains: Array.from(new Set(patterns)),
          action: "block",
          proxy: null,
        });
      }
    } else {
      nextRules.push({
        name: `Block Domains (${procNames[0]})`,
        enabled: true,
        process_names: procNames,
        path_prefixes: [],
        path_contains: [],
        include_children: true,
        dst_cidrs: [],
        dst_ports: [],
        protocols: [],
        domains: Array.from(new Set(patterns)),
        action: "block",
        proxy: null,
      });
    }

    await onSaveConfig(
      { ...config, rules: nextRules },
      t("apps.domainAddedToast", { count: patterns.length })
    );
    setQuickDomainInput("");
    setQuickDomainProc("");
  };

  const removeBlockedDomain = async (ruleIndex: number, pattern: string) => {
    const nextRules: RuleConfig[] = [];
    config.rules.forEach((rule, idx) => {
      if (idx !== ruleIndex) {
        nextRules.push(rule);
        return;
      }
      const remaining = (rule.domains ?? []).filter(
        (d) => d.toLowerCase() !== pattern.toLowerCase()
      );
      if (remaining.length > 0) {
        nextRules.push({ ...rule, domains: remaining });
      }
    });
    await onSaveConfig(
      { ...config, rules: nextRules },
      t("apps.domainRemovedToast", { pattern })
    );
  };

  const toggleRuleEnabled = async (index: number) => {
    const nextRules = config.rules.map((r, i) =>
      i === index ? { ...r, enabled: !r.enabled } : r
    );
    await onSaveConfig({ ...config, rules: nextRules }, t("apps.toastRuleUpdated"));
  };

  const changeRuleProxy = async (index: number, proxyName: string) => {
    const nextRules = config.rules.map((r, i) =>
      i === index ? { ...r, proxy: proxyName } : r
    );
    await onSaveConfig({ ...config, rules: nextRules }, t("apps.toastProxyUpdated"));
  };

  const deleteRule = async (index: number) => {
    const removed = config.rules[index];
    const nextRules = config.rules.filter((_, i) => i !== index);
    await onSaveConfig(
      { ...config, rules: nextRules },
      t("apps.toastRuleRemoved", { name: removed.name })
    );
  };

  const addCustomRule = async (e: React.FormEvent) => {
    e.preventDefault();
    const names = processInput
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const paths = pathInput
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const domains = parseListInput(domainsInput).map((d) =>
      d.replace(/\.+$/, "").toLowerCase()
    );
    if (!ruleName.trim() || (names.length === 0 && paths.length === 0 && domains.length === 0)) {
      return;
    }

    const newRule: RuleConfig = {
      name: ruleName.trim(),
      enabled: true,
      process_names: names,
      path_prefixes: [],
      path_contains: paths,
      include_children: includeChildren,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains,
      action,
      proxy: action === "proxy" ? selectedProxy || defaultProxy : null,
    };

    await onSaveConfig(
      { ...config, rules: [...config.rules, newRule] },
      t("apps.toastRuleAdded", { name: newRule.name })
    );
    setRuleName("");
    setProcessInput("");
    setPathInput("");
    setDomainsInput("");
    setShowComposer(false);
  };

  const quickProxyProcess = async (proc: ProcessEntry) => {
    if (!defaultProxy) return;
    const baseName = proc.name.replace(/\.exe$/i, "");
    const newRule: RuleConfig = {
      name: `${baseName} (Process Tree)`,
      enabled: true,
      process_names: [proc.name],
      path_prefixes: [],
      path_contains: [],
      include_children: true,
      dst_cidrs: [],
      dst_ports: [],
      protocols: [],
      domains: [],
      action: "proxy",
      proxy: defaultProxy,
    };
    await onSaveConfig(
      { ...config, rules: [...config.rules, newRule] },
      t("apps.toastProcAdded", { name: proc.name })
    );
    void loadProcesses();
  };

  const hasComposerCriteria =
    Boolean(processInput.trim()) || Boolean(pathInput.trim()) || Boolean(domainsInput.trim());

  return (
    <div className="view-container">
      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("apps.presetsTitle")}</h2>
            <p className="section-desc">{t("apps.presetsDesc")}</p>
          </div>
        </div>

        <div className="preset-grid">
          {presets.map((preset) => {
            const enabled = isPresetEnabled(preset);
            const i18nKeys = PRESET_I18N_KEYS[preset.id];
            const categoryText = i18nKeys ? t(i18nKeys.category) : preset.category;
            const descText = i18nKeys ? t(i18nKeys.desc) : preset.description;

            return (
              <div
                key={preset.id}
                className={`preset-card ${enabled ? "enabled" : ""}`}
              >
                <div className="preset-card-top">
                  <div>
                    <div className="preset-name">{preset.name}</div>
                    <div className="preset-category">{categoryText}</div>
                  </div>
                  <button
                    type="button"
                    role="switch"
                    aria-checked={enabled}
                    aria-label={preset.name}
                    className="switch"
                    disabled={saving || (!enabled && !defaultProxy)}
                    onClick={() => void togglePreset(preset)}
                  />
                </div>
                <p className="preset-desc">{descText}</p>
                <div className="cell-secondary mono" title={preset.process_names.join(", ")}>
                  {preset.process_names.slice(0, 2).join(", ")}
                  {preset.process_names.length > 2
                    ? ` +${preset.process_names.length - 2}`
                    : ""}
                </div>
              </div>
            );
          })}
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("apps.domainBlockTitle")}</h2>
            <p className="section-desc">{t("apps.domainBlockDesc")}</p>
          </div>
        </div>

        <div className="surface-panel domain-quick-panel">
          <form
            className="domain-quick-form"
            onSubmit={(e) => void addQuickBlockedDomains(e)}
          >
            <div className="search-box" style={{ minWidth: 240 }}>
              <span className="search-icon">
                <GlobeIcon size={14} />
              </span>
              <input
                className="input mono"
                value={quickDomainInput}
                onChange={(e) => setQuickDomainInput(e.target.value)}
                placeholder={t("apps.domainQuickPlaceholder")}
                aria-label={t("apps.domainBlockTitle")}
              />
            </div>

            <input
              className="input mono domain-scope-input"
              value={quickDomainProc}
              onChange={(e) => setQuickDomainProc(e.target.value)}
              placeholder={t("apps.domainQuickProcPlaceholder")}
              aria-label={t("apps.domainQuickScopeAll")}
            />

            <button
              type="submit"
              className="btn btn-primary"
              disabled={saving || parseListInput(quickDomainInput).length === 0}
            >
              <BanIcon size={13} />
              <span>{t("apps.domainQuickAdd")}</span>
            </button>
          </form>

          {blockedDomainEntries.length === 0 ? (
            <p className="domain-empty-hint">{t("apps.domainEmptyHint")}</p>
          ) : (
            <div className="domain-chip-list">
              {blockedDomainEntries.map((item) => (
                <span
                  key={`${item.ruleIndex}-${item.domain}`}
                  className={`domain-chip ${item.enabled ? "" : "muted"}`}
                >
                  <span
                    className={`dot ${item.enabled ? "dot-err" : "dot-idle"}`}
                    aria-hidden="true"
                  />
                  <span className="mono">{item.domain}</span>
                  {item.scope && (
                    <span className="domain-chip-scope">{item.scope}</span>
                  )}
                  <button
                    type="button"
                    className="domain-chip-remove"
                    disabled={saving}
                    aria-label={`Remove ${item.domain}`}
                    onClick={() => void removeBlockedDomain(item.ruleIndex, item.domain)}
                  >
                    <CloseIcon size={11} />
                  </button>
                </span>
              ))}
            </div>
          )}
        </div>
      </section>

      <section>
        <div className="section-head">
          <div>
            <h2 className="section-title">{t("apps.rulesTitle")}</h2>
            <p className="section-desc">{t("apps.rulesDesc")}</p>
          </div>
          <div className="toolbar-group">
            <button
              type="button"
              className={`btn btn-sm ${showPicker ? "btn-secondary" : "btn-outline"}`}
              onClick={() => {
                setShowPicker((v) => !v);
                setShowComposer(false);
              }}
            >
              <TerminalIcon size={13} />
              <span>{t("apps.pickProcess")}</span>
            </button>
            <button
              type="button"
              className={`btn btn-sm ${showComposer ? "btn-secondary" : "btn-primary"}`}
              onClick={() => {
                setShowComposer((v) => !v);
                setShowPicker(false);
              }}
            >
              <PlusIcon size={13} />
              <span>{t("apps.addRule")}</span>
            </button>
          </div>
        </div>

        {showComposer && (
          <form
            className="composer-panel"
            onSubmit={(e) => void addCustomRule(e)}
            style={{ marginBottom: 14 }}
          >
            <div className="form-grid">
              <div className="form-field">
                <label className="form-label" htmlFor="rule-name">
                  {t("apps.ruleName")}
                </label>
                <input
                  id="rule-name"
                  className="input"
                  value={ruleName}
                  onChange={(e) => setRuleName(e.target.value)}
                  placeholder={t("apps.ruleNamePlaceholder")}
                  required
                />
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="rule-action">
                  {t("apps.action")}
                </label>
                <select
                  id="rule-action"
                  className="select"
                  value={action}
                  onChange={(e) => setAction(e.target.value as RuleAction)}
                >
                  <option value="proxy">{t("apps.actionProxy")}</option>
                  <option value="direct">{t("apps.actionDirect")}</option>
                  <option value="block">{t("apps.actionBlock")}</option>
                </select>
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="rule-procs">
                  {t("apps.exeNames")}
                </label>
                <input
                  id="rule-procs"
                  className="input mono"
                  value={processInput}
                  onChange={(e) => setProcessInput(e.target.value)}
                  placeholder={t("apps.exeNamesPlaceholder")}
                />
                <span className="form-hint">{t("apps.exeNamesHint")}</span>
              </div>

              <div className="form-field">
                <label className="form-label" htmlFor="rule-paths">
                  {t("apps.pathContains")}
                </label>
                <input
                  id="rule-paths"
                  className="input mono"
                  value={pathInput}
                  onChange={(e) => setPathInput(e.target.value)}
                  placeholder={t("apps.pathContainsPlaceholder")}
                />
                <span className="form-hint">{t("apps.pathContainsHint")}</span>
              </div>

              <div className="form-field full">
                <label className="form-label" htmlFor="rule-domains">
                  {t("apps.domains")}
                </label>
                <input
                  id="rule-domains"
                  className="input mono"
                  value={domainsInput}
                  onChange={(e) => setDomainsInput(e.target.value)}
                  placeholder={t("apps.domainsPlaceholder")}
                />
                <span className="form-hint">{t("apps.domainsHint")}</span>
              </div>

              {action === "proxy" && (
                <div className="form-field">
                  <label className="form-label" htmlFor="rule-proxy">
                    {t("apps.upstreamProxy")}
                  </label>
                  <select
                    id="rule-proxy"
                    className="select"
                    value={selectedProxy}
                    onChange={(e) => setSelectedProxy(e.target.value)}
                  >
                    {proxyNames.map((name) => (
                      <option key={name} value={name}>
                        {name} ({config.proxies[name].address})
                      </option>
                    ))}
                  </select>
                </div>
              )}

              <div className="form-field" style={{ justifyContent: "center" }}>
                <span className="form-label">{t("apps.childTree")}</span>
                <div style={{ display: "flex", alignItems: "center", gap: 10, height: 34 }}>
                  <button
                    type="button"
                    role="switch"
                    aria-checked={includeChildren}
                    className="switch"
                    onClick={() => setIncludeChildren((v) => !v)}
                  />
                  <span className="form-hint">{t("apps.childTreeHint")}</span>
                </div>
              </div>
            </div>

            <div className="form-actions">
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => setShowComposer(false)}
              >
                {t("apps.cancel")}
              </button>
              <button
                type="submit"
                className="btn btn-primary"
                disabled={saving || !ruleName.trim() || !hasComposerCriteria}
              >
                {t("apps.saveHotReload")}
              </button>
            </div>
          </form>
        )}

        {showPicker && (
          <div className="composer-panel" style={{ marginBottom: 14 }}>
            <div className="toolbar">
              <div className="search-box">
                <span className="search-icon">
                  <SearchIcon size={14} />
                </span>
                <input
                  className="input"
                  type="search"
                  value={procFilter}
                  onChange={(e) => setProcFilter(e.target.value)}
                  placeholder={t("apps.procSearchPlaceholder")}
                />
              </div>
              <div className="toolbar-group">
                <button
                  type="button"
                  className="btn btn-sm btn-outline"
                  onClick={() => void loadProcesses()}
                  disabled={loadingProcs}
                >
                  <RefreshIcon size={13} />
                  <span>{loadingProcs ? t("apps.scanning") : t("apps.refresh")}</span>
                </button>
                <button
                  type="button"
                  className="btn btn-sm btn-ghost"
                  onClick={() => setShowPicker(false)}
                >
                  {t("apps.done")}
                </button>
              </div>
            </div>

            <div className="table-wrap" style={{ maxHeight: 300, overflowY: "auto" }}>
              <table className="data-table">
                <thead>
                  <tr>
                    <th style={{ width: "26%" }}>{t("apps.colProc")}</th>
                    <th style={{ width: "44%" }}>{t("apps.colExePath")}</th>
                    <th style={{ width: "16%" }}>{t("apps.colCoverage")}</th>
                    <th style={{ width: "14%", textAlign: "right" }}>{t("apps.colAction")}</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredProcesses.map((proc) => (
                    <tr key={`${proc.pid}-${proc.name}`}>
                      <td>
                        <div className="cell-primary">{proc.name}</div>
                        <div className="cell-secondary mono">
                          PID {proc.pid} · PPID {proc.parent_pid}
                        </div>
                      </td>
                      <td className="mono cell-secondary" title={proc.exe_path ?? undefined}>
                        {proc.exe_path ?? t("apps.systemProtected")}
                      </td>
                      <td>
                        {proc.matched_rule ? (
                          <span className="tag">
                            <CheckIcon size={11} />
                            <span>{proc.matched_rule.replace(/^Preset:\s*/i, "")}</span>
                          </span>
                        ) : (
                          <span className="cell-secondary">{t("apps.direct")}</span>
                        )}
                      </td>
                      <td style={{ textAlign: "right" }}>
                        <button
                          type="button"
                          className="btn btn-sm btn-outline"
                          disabled={saving || Boolean(proc.matched_rule) || !defaultProxy}
                          onClick={() => void quickProxyProcess(proc)}
                        >
                          {proc.matched_rule ? t("apps.covered") : t("apps.proxyTree")}
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}

        <div className="row-list">
          {config.rules.length === 0 ? (
            <div className="empty-state">
              <h3 className="empty-title">{t("apps.noRulesTitle")}</h3>
              <p className="empty-desc">{t("apps.noRulesDesc")}</p>
            </div>
          ) : (
            config.rules.map((rule, idx) => {
              const domains = rule.domains ?? [];
              return (
                <div key={`${rule.name}-${idx}`} className="row-item">
                  <div className="row-item-main">
                    <div className="row-item-title">
                      {rule.action === "block" && (
                        <span
                          className={`dot ${rule.enabled ? "dot-err" : "dot-idle"}`}
                          aria-hidden="true"
                        />
                      )}
                      <span>{rule.name}</span>
                      {domains.length > 0 && (
                        <span className="tag">{t("apps.tagDomain")}</span>
                      )}
                      {rule.include_children &&
                        (rule.process_names.length > 0 || rule.path_contains.length > 0) && (
                          <span className="tag">{t("apps.tagProcessTree")}</span>
                        )}
                      <span className="tag tag-mono">{rule.action.toUpperCase()}</span>
                    </div>
                    <div className="row-item-sub mono">
                      {[
                        domains.length > 0 ? `domain: ${domains.join(", ")}` : null,
                        rule.process_names.length > 0
                          ? `exe: ${rule.process_names.join(", ")}`
                          : null,
                        rule.path_contains.length > 0
                          ? `path: ${rule.path_contains.join(", ")}`
                          : null,
                      ]
                        .filter(Boolean)
                        .join("  ·  ") || t("apps.allMatchingTraffic")}
                    </div>
                  </div>

                  <div className="row-item-actions">
                    {rule.action === "proxy" && proxyNames.length > 0 && (
                      <select
                        className="select"
                        style={{ height: 28, fontSize: 12.5 }}
                        value={rule.proxy ?? defaultProxy}
                        aria-label={rule.name}
                        onChange={(e) => void changeRuleProxy(idx, e.target.value)}
                      >
                        {proxyNames.map((name) => (
                          <option key={name} value={name}>
                            {name}
                          </option>
                        ))}
                      </select>
                    )}

                    <button
                      type="button"
                      role="switch"
                      aria-checked={rule.enabled}
                      aria-label={rule.name}
                      className="switch"
                      disabled={saving}
                      onClick={() => void toggleRuleEnabled(idx)}
                    />

                    <button
                      type="button"
                      className="icon-btn btn-danger-ghost"
                      title={t("apps.deleteRule")}
                      aria-label={t("apps.deleteRule")}
                      disabled={saving}
                      onClick={() => void deleteRule(idx)}
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
