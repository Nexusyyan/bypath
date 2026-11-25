import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEffect, useState } from "react";
import {
  BrandMark,
  CloseIcon,
  LanguageIcon,
  MaximizeIcon,
  MinimizeIcon,
  RefreshIcon,
  RestoreIcon,
} from "./Icons";
import { isTauriRuntime } from "../lib/api";
import { useI18n } from "../lib/i18n";
import type { DaemonStatus } from "../lib/types";

interface TitlebarProps {
  sectionLabel: string;
  status: DaemonStatus | null;
  firstProxyAddress: string | null;
  onRefresh: () => void;
}

export function Titlebar({
  sectionLabel,
  status,
  firstProxyAddress,
  onRefresh,
}: TitlebarProps) {
  const { locale, toggleLocale, t } = useI18n();
  const [maximized, setMaximized] = useState(false);
  const inTauri = isTauriRuntime();

  useEffect(() => {
    if (!inTauri) return;
    const win = getCurrentWindow();
    void win.isMaximized().then(setMaximized).catch(() => {});
    const unlistenPromise = win.onResized(() => {
      void win.isMaximized().then(setMaximized).catch(() => {});
    });
    return () => {
      void unlistenPromise.then((u) => u());
    };
  }, [inTauri]);

  const handleMinimize = () => {
    if (!inTauri) return;
    void getCurrentWindow().minimize();
  };

  const handleToggleMaximize = () => {
    if (!inTauri) return;
    const win = getCurrentWindow();
    void win.toggleMaximize().then(() => win.isMaximized().then(setMaximized));
  };

  const handleClose = () => {
    if (!inTauri) return;
    void getCurrentWindow().close();
  };

  return (
    <header className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left" data-tauri-drag-region>
        <div className="titlebar-brand" data-tauri-drag-region>
          <BrandMark size={18} />
          <span className="brand-title" data-tauri-drag-region>
            {t("app.name")}
          </span>
          <span className="mode-chip">
            {status?.backend_mode === "service" ? t("mode.service") : t("mode.local")}
          </span>
        </div>

        <span className="titlebar-section" data-tauri-drag-region>
          {sectionLabel}
        </span>
      </div>

      <div className="titlebar-drag-spacer" data-tauri-drag-region />

      <div className="titlebar-right">
        <span className="status-inline" data-tauri-drag-region>
          <span
            className={`dot ${status?.engine_running ? "dot-ok" : "dot-idle"}`}
          />
          <span>
            {status?.engine_running ? t("engine.active") : t("engine.standby")}
            {firstProxyAddress ? ` · ${firstProxyAddress}` : ""}
          </span>
        </span>

        <button
          type="button"
          className="titlebar-pill-btn"
          title={t("topbar.switchLang")}
          aria-label={t("topbar.switchLang")}
          onClick={toggleLocale}
        >
          <LanguageIcon size={14} />
          <span>{locale === "zh-CN" ? "ZH" : "EN"}</span>
        </button>

        <button
          type="button"
          className="icon-btn"
          title={t("topbar.refresh")}
          aria-label={t("topbar.refresh")}
          onClick={onRefresh}
        >
          <RefreshIcon size={14} />
        </button>

        {inTauri && (
          <>
            <span className="titlebar-sep" aria-hidden="true" />
            <div className="window-controls">
              <button
                type="button"
                className="win-btn"
                aria-label="Minimize"
                onClick={handleMinimize}
              >
                <MinimizeIcon size={13} />
              </button>
              <button
                type="button"
                className="win-btn"
                aria-label={maximized ? "Restore" : "Maximize"}
                onClick={handleToggleMaximize}
              >
                {maximized ? <RestoreIcon size={12} /> : <MaximizeIcon size={12} />}
              </button>
              <button
                type="button"
                className="win-btn win-btn-close"
                aria-label="Close"
                onClick={handleClose}
              >
                <CloseIcon size={13} />
              </button>
            </div>
          </>
        )}
      </div>
    </header>
  );
}
