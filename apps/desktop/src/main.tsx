import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { TrayMenu } from "./components/TrayMenu";
import { I18nProvider } from "./lib/i18n";
import { ThemeProvider } from "./lib/theme";
import "./styles/tokens.css";
import "./styles/app.css";

const isTrayView =
  typeof window !== "undefined" &&
  new URLSearchParams(window.location.search).get("view") === "tray";

if (isTrayView && typeof document !== "undefined") {
  document.documentElement.dataset.view = "tray";
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ThemeProvider>
      <I18nProvider>{isTrayView ? <TrayMenu /> : <App />}</I18nProvider>
    </ThemeProvider>
  </StrictMode>
);
