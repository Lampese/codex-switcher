import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import i18next from "i18next";

export type SupportedLanguage = "en-US" | "zh-CN";

const LANGUAGE_STORAGE_KEY = "codex-switcher-language";
type MessageCatalog = Record<string, string>;

const messages: Record<SupportedLanguage, MessageCatalog> = {
  "en-US": {
    settingsTitle: "Settings", Settings: "Settings", Tray: "Tray", "Icon + Session": "Icon + Session",
    "Hourly + Weekly": "Hourly + Weekly", Hidden: "Hidden", "Dock Icon": "Dock Icon", "Show in Dock": "Show in Dock",
    "Menu Bar Only": "Menu Bar Only", Language: "Language", English: "English", "Simplified Chinese": "Simplified Chinese",
    "Codex close method": "Codex close method", "Ask every time": "Ask every time", "Gracefully close": "Gracefully close",
    "Force close": "Force close", "Reopen Codex after close": "Reopen Codex after close", "Reopen desktop app": "Reopen desktop app",
    "Keep closed": "Keep closed", Done: "Done", "Add Account": "Add Account", "ChatGPT Login": "ChatGPT Login",
    "Import File": "Import File", Import: "Import", "Generate Login Link": "Generate Login Link",
    "Waiting for browser login...": "Waiting for browser login...", "Export Slim Text": "Export Slim Text",
    "Import Slim Text": "Import Slim Text", "Export Full Encrypted File": "Export Full Encrypted File",
    "Import Full Encrypted File": "Import Full Encrypted File", "Loading accounts...": "Loading accounts...",
    "Failed to load accounts": "Failed to load accounts", "Open Codex": "Open Codex", "Opening...": "Opening...",
    "Refresh all usage": "Refresh all usage", "Warming up all accounts": "Warming up all accounts",
    "Warm up all accounts": "Warm up all accounts", "Auto Warm Up": "Auto Warm Up", Timer: "Timer", Appearance: "Appearance",
    "Timed warm-up": "Timed warm-up", Menu: "Menu", "Exporting...": "Exporting...", "Importing...": "Importing...",
    "Switching...": "Switching...", Switch: "Switch", "Remove account": "Remove account", "Refresh usage": "Refresh usage",
    "Show info": "Show info", "Hide info": "Hide info", "Show usage statistics": "Show usage statistics",
    "Hide usage statistics": "Hide usage statistics", "API Key": "API Key", Unknown: "Unknown", Never: "Never",
    "Just now": "Just now", "Expiry unavailable": "Expiry unavailable", "No expiry": "No expiry", "Token activity": "Token activity",
    "Fast mode": "Fast mode", Reasoning: "Reasoning", "Skills explored": "Skills explored", "Total threads": "Total threads",
    Lifetime: "Lifetime", Today: "Today", "Last 7 days": "Last 7 days", "Last 30 days": "Last 30 days",
    "Last 3 months": "Last 3 months", "Last 6 months": "Last 6 months", "Current streak": "Current streak",
    "Longest task": "Longest task", "Longest streak": "Longest streak", "Peak day": "Peak day", "All reported": "All reported",
    All: "All", reported: "reported", tokens: "tokens", days: "days", "Usage stats unavailable.": "Usage stats unavailable.",
    "No accounts configured": "No accounts configured", "Open Codex Switcher": "Open Codex Switcher", "Codex Switcher": "Codex Switcher", Quit: "Quit",
    "Loading...": "Loading...", Session: "Session", Weekly: "Weekly", "Resets now": "Resets now", "Usage unavailable": "Usage unavailable",
    today: "today", "last 7 days": "last 7 days", Account: "Account", "Click to rename": "Click to rename",
    File: "File", Edit: "Edit", View: "View", Window: "Window", Help: "Help",
  },
  "zh-CN": {
    settingsTitle: "设置", Settings: "设置", Tray: "托盘", "Icon + Session": "图标 + 会话", "Hourly + Weekly": "每小时 + 每周",
    Hidden: "隐藏", "Dock Icon": "Dock 图标", "Show in Dock": "在 Dock 中显示", "Menu Bar Only": "仅菜单栏", Language: "语言",
    English: "英语", "Simplified Chinese": "简体中文", "Codex close method": "Codex 关闭方式", "Ask every time": "每次询问",
    "Gracefully close": "正常关闭", "Force close": "强制关闭", "Reopen Codex after close": "关闭后重新打开 Codex",
    "Reopen desktop app": "重新打开桌面应用", "Keep closed": "保持关闭", Done: "完成", "Add Account": "添加账号",
    "ChatGPT Login": "ChatGPT 登录", "Import File": "导入文件", Import: "导入", "Generate Login Link": "生成登录链接",
    "Waiting for browser login...": "等待浏览器登录……", "Export Slim Text": "导出精简文本", "Import Slim Text": "导入精简文本",
    "Export Full Encrypted File": "导出完整加密文件", "Import Full Encrypted File": "导入完整加密文件", "Loading accounts...": "正在加载账号……",
    "Failed to load accounts": "加载账号失败", "Open Codex": "打开 Codex", "Opening...": "正在打开……", "Refresh all usage": "刷新全部用量",
    "Warming up all accounts": "正在预热全部账号", "Warm up all accounts": "预热全部账号", "Auto Warm Up": "自动预热", Timer: "定时器",
    Appearance: "外观", "Timed warm-up": "定时预热", Menu: "菜单", "Exporting...": "正在导出……", "Importing...": "正在导入……",
    "Switching...": "正在切换……", Switch: "切换", "Remove account": "移除账号", "Refresh usage": "刷新用量", "Show info": "显示信息",
    "Hide info": "隐藏信息", "Show usage statistics": "显示用量统计", "Hide usage statistics": "隐藏用量统计", "API Key": "API 密钥",
    Unknown: "未知", Never: "从未", "Just now": "刚刚", "Expiry unavailable": "无法获取过期时间", "No expiry": "无过期时间",
    "Token activity": "令牌活动", "Fast mode": "快速模式", Reasoning: "推理", "Skills explored": "探索的技能", "Total threads": "线程总数",
    Lifetime: "全部时间", Today: "今天", "Last 7 days": "最近 7 天", "Last 30 days": "最近 30 天", "Last 3 months": "最近 3 个月",
    "Last 6 months": "最近 6 个月", "Current streak": "当前连续天数", "Longest task": "最长任务", "Longest streak": "最长连续天数",
    "Peak day": "峰值日期", "All reported": "全部已报告", All: "全部", reported: "已报告", tokens: "令牌", days: "天",
    "Usage stats unavailable.": "用量统计不可用。",
    "No accounts configured": "未配置账号", "Open Codex Switcher": "打开 Codex Switcher", "Codex Switcher": "Codex Switcher", Quit: "退出",
    "Loading...": "正在加载……", Session: "会话", Weekly: "每周", "Resets now": "现在重置", "Usage unavailable": "用量不可用",
    today: "今天", "last 7 days": "最近 7 天", Account: "账号", "Click to rename": "点击重命名",
    File: "文件", Edit: "编辑", View: "视图", Window: "窗口", Help: "帮助",
  },
};

const i18n = i18next.createInstance({
  resources: {
    "en-US": { translation: messages["en-US"] },
    "zh-CN": { translation: messages["zh-CN"] },
  },
  fallbackLng: "en-US",
  interpolation: { escapeValue: false },
});
void i18n.init();

type LanguageContextValue = {
  language: SupportedLanguage;
  setLanguage: (language: SupportedLanguage) => void;
  t: (key: string) => string;
};

const LanguageContext = createContext<LanguageContextValue | null>(null);

export function resolveInitialLanguage(language?: string): SupportedLanguage {
  return language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function readBrowserLanguage(): SupportedLanguage | null {
  try {
    const stored = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
    return stored ? resolveInitialLanguage(stored) : null;
  } catch {
    return null;
  }
}

export function getInitialLanguage(): SupportedLanguage {
  if (typeof window !== "undefined" && !isTauriRuntime()) {
    return readBrowserLanguage() ?? resolveInitialLanguage(navigator.language);
  }
  return resolveInitialLanguage(typeof navigator === "undefined" ? undefined : navigator.language);
}

export function translate(key: string, language = getInitialLanguage()): string {
  return i18n.t(key, { lng: language, defaultValue: key });
}

export function translateMessage(
  key: string,
  variables: Record<string, string | number> = {},
  language = getInitialLanguage()
): string {
  return i18n.t(key, { lng: language, defaultValue: key, ...variables });
}

export function translateText(source: string, language: SupportedLanguage): string {
  const leading = source.match(/^\s*/)?.[0] ?? "";
  const trailing = source.match(/\s*$/)?.[0] ?? "";
  const core = source.slice(leading.length, source.length - trailing.length || undefined);
  return `${leading}${translate(core, language)}${trailing}`;
}

type LocalizedState = { source: string; translated: string };
const textState = new WeakMap<Text, LocalizedState>();
const attributeState = new WeakMap<Element, Map<string, LocalizedState>>();

function shouldSkip(element: Element | null): boolean {
  return Boolean(element?.closest("[data-i18n-ignore], script, style, textarea"));
}

function localizeDocument(language: SupportedLanguage): void {
  if (typeof document === "undefined" || !document.body) return;
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  let node: Text | null;
  while ((node = walker.nextNode() as Text | null)) {
    if (shouldSkip(node.parentElement)) continue;
    const current = node.nodeValue ?? "";
    const previous = textState.get(node);
    const source = previous && current === previous.translated ? previous.source : current;
    const translated = translateText(source, language);
    textState.set(node, { source, translated });
    if (current !== translated) node.nodeValue = translated;
  }

  for (const element of Array.from(document.body.querySelectorAll<HTMLElement>("*"))) {
    if (shouldSkip(element)) continue;
    const states = attributeState.get(element) ?? new Map<string, LocalizedState>();
    for (const attribute of ["title", "aria-label", "placeholder", "alt"]) {
      const current = element.getAttribute(attribute);
      if (!current) continue;
      const previous = states.get(attribute);
      const source = previous && current === previous.translated ? previous.source : current;
      const translated = translateText(source, language);
      states.set(attribute, { source, translated });
      if (current !== translated) element.setAttribute(attribute, translated);
    }
    attributeState.set(element, states);
  }
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [language, setLanguageState] = useState<SupportedLanguage>(getInitialLanguage);
  const languageRef = useRef(language);

  useEffect(() => {
    languageRef.current = language;
    void i18n.changeLanguage(language);
    localizeDocument(language);
  }, [language]);

  useEffect(() => {
    let disposed = false;
    if (isTauriRuntime()) {
      void import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke<string>("get_language"))
        .then((value) => {
          if (!disposed) setLanguageState(resolveInitialLanguage(value));
        })
        .catch(() => undefined);
    }
    let localizationScheduled = false;
    const observer = new MutationObserver(() => {
      if (localizationScheduled) return;
      localizationScheduled = true;
      queueMicrotask(() => {
        localizationScheduled = false;
        localizeDocument(languageRef.current);
      });
    });
    observer.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true });
    let stop: (() => void) | undefined;
    if (isTauriRuntime()) {
      void import("@tauri-apps/api/event").then(async ({ listen }) => {
        stop = await listen("app-settings-changed", () => {
          void import("@tauri-apps/api/core")
            .then(({ invoke }) => invoke<string>("get_language"))
            .then((value) => {
              if (!disposed) setLanguageState(resolveInitialLanguage(value));
            })
            .catch(() => undefined);
        });
      }).catch(() => undefined);
    }
    return () => {
      disposed = true;
      observer.disconnect();
      stop?.();
    };
  }, []);

  const setLanguage = useCallback((next: SupportedLanguage) => {
    setLanguageState(next);
    if (!isTauriRuntime()) {
      try {
        window.localStorage.setItem(LANGUAGE_STORAGE_KEY, next);
      } catch {
        // Use the in-memory browser preference when storage is unavailable.
      }
    }
  }, []);
  const value = useMemo(() => ({ language, setLanguage, t: (key: string) => translate(key, language) }), [language, setLanguage]);

  return createElement(LanguageContext.Provider, { value }, children);
}

export function useI18n(): LanguageContextValue {
  const context = useContext(LanguageContext);
  if (!context) throw new Error("useI18n must be used inside I18nProvider");
  return context;
}
