import { useI18n } from "../lib/i18n";
import type { DaemonStatus, ProxyRunMode } from "../lib/types";

interface ModeSwitcherProps {
  mode: ProxyRunMode;
  switchingTo: ProxyRunMode | null;
  status: DaemonStatus | null;
  compact?: boolean;
  onSelectMode: (target: ProxyRunMode) => void;
}

export function ModeSwitcher({
  mode,
  switchingTo,
  status,
  compact = false,
  onSelectMode,
}: ModeSwitcherProps) {
  const { t } = useI18n();
  const busy = switchingTo !== null;

  const fgTooltip = status?.elevated
    ? t("mode.tipForegroundReady")
    : t("mode.tipForegroundUac");
  const bgTooltip = status?.service_running
    ? t("mode.tipBackgroundReady")
    : status?.service_installed
      ? t("mode.tipBackgroundStart")
      : t("mode.tipBackgroundInstall");

  const caption = busy
    ? switchingTo === "off"
      ? t("mode.switchingOff")
      : switchingTo === "foreground"
        ? status?.elevated
          ? t("mode.switchingForeground")
          : t("mode.switchingForegroundUac")
        : t("mode.switchingBackground")
    : mode === "background"
      ? t("mode.captionBackground")
      : mode === "foreground"
        ? t("mode.captionForeground")
        : t("mode.captionOff");

  const dotClass = busy ? "dot-warn" : mode === "off" ? "dot-idle" : "dot-ok";

  const options: Array<{ id: ProxyRunMode; label: string; title: string }> = [
    { id: "off", label: t("mode.off"), title: t("mode.tipOff") },
    { id: "foreground", label: t("mode.foreground"), title: fgTooltip },
    { id: "background", label: t("mode.background"), title: bgTooltip },
  ];

  return (
    <div className={`mode-switcher-wrap ${compact ? "compact" : ""}`}>
      <div
        className={`mode-switcher ${compact ? "compact" : ""}`}
        role="radiogroup"
        aria-label={t("mode.ariaLabel")}
      >
        {options.map((opt) => {
          const selected = mode === opt.id;
          const isRunningOption = selected && opt.id !== "off";
          const isPending = switchingTo === opt.id;
          return (
            <button
              key={opt.id}
              type="button"
              role="radio"
              aria-checked={selected}
              data-tray-focusable={compact ? "true" : undefined}
              className={`mode-switcher-btn ${
                selected ? (isRunningOption ? "active-live" : "active-off") : ""
              } ${isPending ? "pending" : ""}`}
              disabled={busy}
              title={opt.title}
              onClick={() => {
                if (!selected && !busy) {
                  onSelectMode(opt.id);
                }
              }}
            >
              {isRunningOption && <span className="mode-live-dot" aria-hidden="true" />}
              <span>{opt.label}</span>
            </button>
          );
        })}
      </div>

      {!compact && (
        <div className="mode-switcher-caption" aria-live="polite">
          <span className={`dot ${dotClass}`} aria-hidden="true" />
          <span className="mode-switcher-caption-text">{caption}</span>
        </div>
      )}
    </div>
  );
}
