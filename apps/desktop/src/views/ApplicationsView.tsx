import { useEffect, useMemo, useState } from "react";
import {
  ArrowDownIcon,
  ArrowUpIcon,
  BanIcon,
  CheckIcon,
  CloseIcon,
  EditIcon,
  GlobeIcon,
  PlusIcon,
  RefreshIcon,
  SearchIcon,
  TerminalIcon,
  TestIcon,
  TrashIcon,
} from "../components/Icons";
import { Select } from "../components/Select";
import { api } from "../lib/api";
import { useI18n, type MessageKey } from "../lib/i18n";
import type {
  Config,
  PresetInfo,
  ProcessEntry,
  RuleAction,
  RuleConfig,
  RuleTestReport,
} from "../lib/types";

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

function parseCommaList(raw: string): string[] {
  return raw
    .split(/[,\n]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

function parsePortList(raw: string): number[] {
  return parseListInput(raw)
    .map((s) => Number(s))
    .filter((n) => Number.isInteger(n) && n >= 1 && n <= 65535);
}

export function ApplicationsView({
  config,
  presets,
  saving,
  onSaveConfig,
}: ApplicationsViewProps) {
  const { t } = useI18n();
  const [showComposer, setShowComposer] = useState(false);
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [showPicker, setShowPicker] = useState(false);
  const [showTester, setShowTester] = useState(false);

  const proxyNames = Object.keys(config.proxies);
  const defaultProxy = proxyNames[0] ?? "";

  const [quickDomainInput, setQuickDomainInput] = useState("");
  const [quickDomainProc, setQuickDomainProc] = useState("");

  // Rule composer fields
  const [ruleName, setRuleName] = useState("");
  const [processInput, setProcessInput] = useState("");
  const [pathInput, setPathInput] = useState("");
  const [pathPrefixInput, setPathPrefixInput] = useState("");
  const [domainsInput, setDomainsInput] = useState("");
  const [portsInput, setPortsInput] = useState("");
  const [cidrsInput, setCidrsInput] = useState("");
  const [protoChoice, setProtoChoice] = useState<"all" | "tcp" | "udp">("all");
  const [includeChildren, setIncludeChildren] = useState(true);
  const [action, setAction] = useState<RuleAction>("proxy");
  const [selectedProxy, setSelectedProxy] = useState(defaultProxy);

  // Process picker state
  const [processes, setProcesses] = useState<ProcessEntry[]>([]);
  const [procFilter, setProcFilter] = useState("");
  const [loadingProcs, setLoadingProcs] = useState(false);

  // Rule dry-run tester state
  const [testProcess, setTestProcess] = useState("Antigravity.exe");
  const [testPath, setTestPath] = useState("");
  const [testHost, setTestHost] = useState("aicode.googleapis.com");
  const [testPort, setTestPort] = useState("443");
  const [testProto, setTestProto] = useState<"tcp" | "udp">("tcp");
  const [testingRule, setTestingRule] = useState(false);
  const [testReport, setTestReport] = useState<RuleTestReport | null>(null);

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

  const resetComposer = () => {
    setEditingIndex(null);
    setRuleName("");
    setProcessInput("");
    setPathInput("");
    setPathPrefixInput("");
    setDomainsInput("");
    setPortsInput("");
    setCidrsInput("");
    setProtoChoice("all");
    setIncludeChildren(true);
    setAction("proxy");
    setSelectedProxy(defaultProxy);
    setShowAdvanced(false);
  };

  const openNewComposer = () => {
    if (showComposer && editingIndex === null) {
      setShowComposer(false);
      return;
    }
    resetComposer();
    setShowComposer(true);
    setShowPicker(false);
    setShowTester(false);
  };

  const openEditComposer = (index: number) => {
    const r = config.rules[index];
    setEditingIndex(index);
    setRuleName(r.name);
    setProcessInput(r.process_names.join(", "));
    setPathInput(r.path_contains.join(", "));
    setPathPrefixInput((r.path_prefixes ?? []).join(", "));
    setDomainsInput((r.domains ?? []).join(", "));
    setPortsInput((r.dst_ports ?? []).join(", "));
    setCidrsInput((r.dst_cidrs ?? []).join(", "));
    const protos = (r.protocols ?? []).map((p) => p.toLowerCase());
    setProtoChoice(
      protos.length === 1 && protos[0] === "tcp"
        ? "tcp"
        : protos.length === 1 && protos[0] === "udp"
          ? "udp"
          : "all"
    );
    setIncludeChildren(r.include_children);
    setAction(r.action);
    setSelectedProxy(r.proxy ?? defaultProxy);
    const hasAdvanced =
      (r.path_prefixes ?? []).length > 0 ||
      (r.dst_ports ?? []).length > 0 ||
      (r.dst_cidrs ?? []).length > 0 ||
      protos.length > 0;
    setShowAdvanced(hasAdvanced);
    setShowComposer(true);
    setShowPicker(false);
    setShowTester(false);
  };

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

    const procNames = parseCommaList(quickDomainProc);
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

  const moveRule = async (index: number, delta: -1 | 1) => {
    const target = index + delta;
    if (target < 0 || target >= config.rules.length) return;
    const nextRules = [...config.rules];
    const [item] = nextRules.splice(index, 1);
    nextRules.splice(target, 0, item);
    if (editingIndex === index) {
      setEditingIndex(target);
    }
    await onSaveConfig({ ...config, rules: nextRules }, t("apps.toastOrderUpdated"));
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
    if (editingIndex === index) {
      setShowComposer(false);
      setEditingIndex(null);
    }
    await onSaveConfig(
      { ...config, rules: nextRules },
      t("apps.toastRuleRemoved", { name: removed.name })
    );
  };

  const saveRuleForm = async (e: React.FormEvent) => {
    e.preventDefault();
    const names = parseCommaList(processInput);
    const paths = parseCommaList(pathInput);
    const prefixes = parseCommaList(pathPrefixInput);
    const domains = parseListInput(domainsInput).map((d) =>
      d.replace(/\.+$/, "").toLowerCase()
    );
    const dstPorts = parsePortList(portsInput);
    const dstCidrs = parseListInput(cidrsInput);
    const protocols = protoChoice === "all" ? [] : [protoChoice];

    if (
      !ruleName.trim() ||
      (names.length === 0 &&
        paths.length === 0 &&
        prefixes.length === 0 &&
        domains.length === 0 &&
        dstPorts.length === 0 &&
        dstCidrs.length === 0)
    ) {
      return;
    }

    const existingEnabled = editingIndex !== null ? config.rules[editingIndex].enabled : true;
    const builtRule: RuleConfig = {
      name: ruleName.trim(),
      enabled: existingEnabled,
      process_names: names,
      path_prefixes: prefixes,
      path_contains: paths,
      include_children: includeChildren,
      dst_cidrs: dstCidrs,
      dst_ports: dstPorts,
      protocols,
      domains,
      action,
      proxy: action === "proxy" ? selectedProxy || defaultProxy : null,
    };

    const nextRules = [...config.rules];
    if (editingIndex !== null && editingIndex < nextRules.length) {
      nextRules[editingIndex] = builtRule;
      await onSaveConfig({ ...config, rules: nextRules }, t("apps.toastRuleUpdated"));
    } else {
      nextRules.push(builtRule);
      await onSaveConfig(
        { ...config, rules: nextRules },
        t("apps.toastRuleAdded", { name: builtRule.name })
      );
    }

    resetComposer();
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

  const runDryRunTest = async (e: React.FormEvent) => {
    e.preventDefault();
    setTestingRule(true);
    try {
      const portNum = Number(testPort) || 443;
      const report = await api.testRule({
        process_name: testProcess.trim() || null,
        exe_path: testPath.trim() || null,
        host: testHost.trim() || null,
        dst_ip: null,
        dst_port: portNum,
        protocol: testProto,
      });
      setTestReport(report);
    } finally {
      setTestingRule(false);
    }
  };

  const hasComposerCriteria =
    Boolean(processInput.trim()) ||
    Boolean(pathInput.trim()) ||
    Boolean(pathPrefixInput.trim()) ||
    Boolean(domainsInput.trim()) ||
    Boolean(portsInput.trim()) ||
    Boolean(cidrsInput.trim());

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
              className={`btn btn-sm ${showTester ? "btn-secondary" : "btn-outline"}`}
              onClick={() => {
                setShowTester((v) => !v);
                setShowComposer(false);
                setShowPicker(false);
              }}
            >
              <TestIcon size={13} />
              <span>{t("apps.testMatch")}</span>
            </button>
            <button
              type="button"
              className={`btn btn-sm ${showPicker ? "btn-secondary" : "btn-outline"}`}
              onClick={() => {
                setShowPicker((v) => !v);
                setShowComposer(false);
                setShowTester(false);
              }}
            >
              <TerminalIcon size={13} />
              <span>{t("apps.pickProcess")}</span>
            </button>
            <button
              type="button"
              className={`btn btn-sm ${showComposer && editingIndex === null ? "btn-secondary" : "btn-primary"}`}
              onClick={openNewComposer}
            >
              <PlusIcon size={13} />
              <span>{t("apps.addRule")}</span>
            </button>
          </div>
        </div>

        {showTester && (
          <form
            className="composer-panel"
            onSubmit={(e) => void runDryRunTest(e)}
            style={{ marginBottom: 14 }}
          >
            <div className="form-grid">
              <div className="form-field">
                <label className="form-label" htmlFor="test-proc">
                  {t("apps.testProcess")}
                </label>
                <input
                  id="test-proc"
                  className="input mono"
                  value={testProcess}
                  onChange={(e) => setTestProcess(e.target.value)}
                  placeholder="Antigravity.exe"
                />
              </div>
              <div className="form-field">
                <label className="form-label" htmlFor="test-host">
                  {t("apps.testHost")}
                </label>
                <input
                  id="test-host"
                  className="input mono"
                  value={testHost}
                  onChange={(e) => setTestHost(e.target.value)}
                  placeholder="aicode.googleapis.com"
                />
              </div>
              <div className="form-field">
                <label className="form-label" htmlFor="test-path">
                  {t("apps.testPath")}
                </label>
                <input
                  id="test-path"
                  className="input mono"
                  value={testPath}
                  onChange={(e) => setTestPath(e.target.value)}
                  placeholder="C:\Program Files\App\app.exe"
                />
              </div>
              <div className="form-field">
                <label className="form-label" htmlFor="test-port">
                  {t("apps.testPort")} / {t("apps.protocols")}
                </label>
                <div style={{ display: "flex", gap: 8 }}>
                  <input
                    id="test-port"
                    className="input mono"
                    style={{ width: 110 }}
                    value={testPort}
                    onChange={(e) => setTestPort(e.target.value)}
                  />
                  <Select<"tcp" | "udp">
                    value={testProto}
                    ariaLabel={t("apps.protocols")}
                    style={{ minWidth: 96 }}
                    onChange={(val) => setTestProto(val)}
                    options={[
                      { value: "tcp", label: "TCP" },
                      { value: "udp", label: "UDP" },
                    ]}
                  />
                </div>
              </div>
            </div>

            {testReport && (
              <div className="test-result-box">
                <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                  <span
                    className={`dot ${
                      testReport.final_action === "proxy"
                        ? "dot-ok"
                        : testReport.final_action === "block"
                          ? "dot-err"
                          : "dot-idle"
                    }`}
                  />
                  <strong>{t("apps.testFinal")}:</strong>
                  <span className="tag tag-mono">{testReport.final_action.toUpperCase()}</span>
                  {testReport.final_proxy && (
                    <span className="tag tag-mono">{testReport.final_proxy}</span>
                  )}
                  {testReport.matched_rule && (
                    <span className="cell-secondary">{testReport.matched_rule}</span>
                  )}
                </div>
                <div className="cell-secondary mono">
                  {t("apps.testPhase1")}: {testReport.phase1_decision} ({testReport.phase1_reason})
                </div>
                {testReport.phase2_decision && (
                  <div className="cell-secondary mono">
                    {t("apps.testPhase2")}: {testReport.phase2_decision} (
                    {testReport.phase2_rule ?? "-"})
                  </div>
                )}
              </div>
            )}

            <div className="form-actions">
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => setShowTester(false)}
              >
                {t("apps.done")}
              </button>
              <button type="submit" className="btn btn-primary" disabled={testingRule}>
                {testingRule ? t("apps.testRunning") : t("apps.testRun")}
              </button>
            </div>
          </form>
        )}

        {showComposer && (
          <form
            className="composer-panel"
            onSubmit={(e) => void saveRuleForm(e)}
            style={{ marginBottom: 14 }}
          >
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
              <span className="section-title" style={{ fontSize: 14 }}>
                {editingIndex !== null ? t("apps.editRuleTitle") : t("apps.newRuleTitle")}
              </span>
              {editingIndex !== null && (
                <span className="priority-badge">#{editingIndex + 1}</span>
              )}
            </div>

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
                <Select<RuleAction>
                  id="rule-action"
                  value={action}
                  ariaLabel={t("apps.action")}
                  onChange={(val) => setAction(val)}
                  options={[
                    { value: "proxy", label: t("apps.actionProxy") },
                    { value: "direct", label: t("apps.actionDirect") },
                    { value: "block", label: t("apps.actionBlock") },
                  ]}
                />
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
                  <Select
                    id="rule-proxy"
                    value={selectedProxy}
                    ariaLabel={t("apps.upstreamProxy")}
                    onChange={(val) => setSelectedProxy(val)}
                    options={proxyNames.map((name) => ({
                      value: name,
                      label: `${name} (${config.proxies[name].address})`,
                    }))}
                  />
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

            <div>
              <button
                type="button"
                className="advanced-toggle"
                onClick={() => setShowAdvanced((v) => !v)}
              >
                <span>{showAdvanced ? t("apps.advancedHide") : t("apps.advancedShow")}</span>
              </button>

              {showAdvanced && (
                <div className="form-grid" style={{ marginTop: 10 }}>
                  <div className="form-field">
                    <label className="form-label" htmlFor="rule-ports">
                      {t("apps.dstPorts")}
                    </label>
                    <input
                      id="rule-ports"
                      className="input mono"
                      value={portsInput}
                      onChange={(e) => setPortsInput(e.target.value)}
                      placeholder={t("apps.dstPortsPlaceholder")}
                    />
                  </div>
                  <div className="form-field">
                    <label className="form-label" htmlFor="rule-proto">
                      {t("apps.protocols")}
                    </label>
                    <Select<"all" | "tcp" | "udp">
                      id="rule-proto"
                      value={protoChoice}
                      ariaLabel={t("apps.protocols")}
                      onChange={(val) => setProtoChoice(val)}
                      options={[
                        { value: "all", label: t("apps.protocolsAll") },
                        { value: "tcp", label: t("apps.protocolsTcp") },
                        { value: "udp", label: t("apps.protocolsUdp") },
                      ]}
                    />
                  </div>
                  <div className="form-field">
                    <label className="form-label" htmlFor="rule-cidrs">
                      {t("apps.dstCidrs")}
                    </label>
                    <input
                      id="rule-cidrs"
                      className="input mono"
                      value={cidrsInput}
                      onChange={(e) => setCidrsInput(e.target.value)}
                      placeholder={t("apps.dstCidrsPlaceholder")}
                    />
                  </div>
                  <div className="form-field">
                    <label className="form-label" htmlFor="rule-prefixes">
                      {t("apps.pathPrefixes")}
                    </label>
                    <input
                      id="rule-prefixes"
                      className="input mono"
                      value={pathPrefixInput}
                      onChange={(e) => setPathPrefixInput(e.target.value)}
                      placeholder={t("apps.pathPrefixesPlaceholder")}
                    />
                  </div>
                </div>
              )}
            </div>

            <div className="form-actions">
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => {
                  resetComposer();
                  setShowComposer(false);
                }}
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
                          PID {proc.pid} / PPID {proc.parent_pid}
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
              const ports = rule.dst_ports ?? [];
              const cidrs = rule.dst_cidrs ?? [];
              const protos = rule.protocols ?? [];
              return (
                <div key={`${rule.name}-${idx}`} className="row-item">
                  <div className="row-item-main">
                    <div className="row-item-title">
                      <span className="priority-badge">#{idx + 1}</span>
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
                        (rule.process_names.length > 0 ||
                          rule.path_contains.length > 0 ||
                          (rule.path_prefixes ?? []).length > 0) && (
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
                        (rule.path_prefixes ?? []).length > 0
                          ? `prefix: ${rule.path_prefixes.join(", ")}`
                          : null,
                        ports.length > 0 ? `port: ${ports.join(", ")}` : null,
                        cidrs.length > 0 ? `cidr: ${cidrs.join(", ")}` : null,
                        protos.length > 0 ? `proto: ${protos.join("/")}` : null,
                      ]
                        .filter(Boolean)
                        .join("  |  ") || t("apps.allMatchingTraffic")}
                    </div>
                  </div>

                  <div className="row-item-actions">
                    <div className="order-btn-group">
                      <button
                        type="button"
                        className="order-btn"
                        disabled={saving || idx === 0}
                        title={t("apps.moveUp")}
                        aria-label={t("apps.moveUp")}
                        onClick={() => void moveRule(idx, -1)}
                      >
                        <ArrowUpIcon size={13} />
                      </button>
                      <button
                        type="button"
                        className="order-btn"
                        disabled={saving || idx === config.rules.length - 1}
                        title={t("apps.moveDown")}
                        aria-label={t("apps.moveDown")}
                        onClick={() => void moveRule(idx, 1)}
                      >
                        <ArrowDownIcon size={13} />
                      </button>
                    </div>

                    {rule.action === "proxy" && proxyNames.length > 0 && (
                      <Select
                        size="sm"
                        align="right"
                        value={rule.proxy ?? defaultProxy}
                        ariaLabel={rule.name}
                        onChange={(val) => void changeRuleProxy(idx, val)}
                        options={proxyNames.map((name) => ({
                          value: name,
                          label: name,
                        }))}
                      />
                    )}

                    <button
                      type="button"
                      className="icon-btn"
                      title={t("apps.editRule")}
                      aria-label={t("apps.editRule")}
                      disabled={saving}
                      onClick={() => openEditComposer(idx)}
                    >
                      <EditIcon size={13} />
                    </button>

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
