import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import enMessages from "../locales/en.json";
import zhCnMessages from "../locales/zh-CN.json";
import { api } from "./api";

export type Locale = "zh-CN" | "en";
export type MessageKey = keyof typeof enMessages;

const STORAGE_KEY = "bypath-locale";

const DICTIONARIES: Record<Locale, Record<MessageKey, string>> = {
  en: enMessages,
  "zh-CN": zhCnMessages,
};

function detectInitialLocale(): Locale {
  if (typeof window !== "undefined") {
    const param = new URLSearchParams(window.location.search)
      .get("lang")
      ?.toLowerCase();
    if (param === "en") return "en";
    if (param === "zh" || param === "zh-cn") return "zh-CN";

    try {
      const saved = window.localStorage.getItem(STORAGE_KEY);
      if (saved === "en" || saved === "zh-CN") return saved;
    } catch {
      // Ignore storage access errors in restricted environments.
    }

    if (navigator.language.toLowerCase().startsWith("zh")) {
      return "zh-CN";
    }
  }
  return "en";
}

interface I18nContextValue {
  locale: Locale;
  setLocale: (next: Locale) => void;
  toggleLocale: () => void;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
}

const I18nContext = createContext<I18nContextValue | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(detectInitialLocale);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY && (e.newValue === "en" || e.newValue === "zh-CN")) {
        setLocaleState(e.newValue);
      }
    };
    window.addEventListener("storage", onStorage);

    let unlisten: (() => void) | null = null;
    void api
      .subscribeUiPrefs((prefs) => {
        if (prefs.locale === "en" || prefs.locale === "zh-CN") {
          setLocaleState(prefs.locale);
        }
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      window.removeEventListener("storage", onStorage);
      unlisten?.();
    };
  }, []);

  const setLocale = useCallback((next: Locale) => {
    setLocaleState(next);
    try {
      window.localStorage.setItem(STORAGE_KEY, next);
    } catch {
      // Ignore storage errors.
    }
    void api.syncUiPrefs({ locale: next });
  }, []);

  const toggleLocale = useCallback(() => {
    setLocale(locale === "zh-CN" ? "en" : "zh-CN");
  }, [locale, setLocale]);

  const t = useCallback(
    (key: MessageKey, params?: Record<string, string | number>): string => {
      const template = DICTIONARIES[locale][key] ?? DICTIONARIES.en[key] ?? key;
      if (!params) return template;
      return template.replace(/\{(\w+)\}/g, (_, name: string) =>
        name in params ? String(params[name]) : `{${name}}`
      );
    },
    [locale]
  );

  const value = useMemo(
    () => ({ locale, setLocale, toggleLocale, t }),
    [locale, setLocale, toggleLocale, t]
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const ctx = useContext(I18nContext);
  if (!ctx) {
    throw new Error("useI18n must be used inside I18nProvider");
  }
  return ctx;
}
