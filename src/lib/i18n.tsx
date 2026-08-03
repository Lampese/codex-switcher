import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { isTauriRuntime } from "./platform";
import type { AppLocale } from "./dateFormat";

export type Language = "en" | "zh";

export const LANGUAGE_STORAGE_KEY = "codex-switcher-language";
export const LANGUAGE_CHANGED_EVENT = "language-changed";

const messages: Record<Language, Record<string, string>> = {
  en: {
    "language.switch": "Switch language",
    "language.english": "English",
    "language.chinese": "中文",
    "process.running": "{{count}} Codex running",
    "process.none": "0 Codex running",
    "forceClose": "Force close",
    "openCodex": "Open Codex",
    "openingCodex": "Opening...",
    "showAll": "Show all account names and emails",
    "hideAll": "Hide all account names and emails",
    "refreshAll": "Refresh all usage",
    "refreshingAll": "Refreshing all usage",
    "sendAllTraffic": "Send minimal traffic using all accounts",
    "enableAutoAll": "Enable auto warm-up for all accounts",
    "disableAutoAll": "Disable auto warm-up for all accounts",
    "scheduleWarmup": "Schedule warm-up at specific times of day for all accounts",
    "timedWarming": "Timed warming...",
    "timedOff": "Timed: off",
    "timedNext": "Timed: {{time}}",
    "timedWarmup": "Timed warm-up",
    "noTimes": "No times added yet.",
    add: "Add",
    remove: "Remove {{time}}",
    "switchToLight": "Switch to light mode",
    "switchToDark": "Switch to dark mode",
    "accountMenu": "Account",
    addAccount: "+ Add Account",
    exportSlim: "Export Slim Text",
    importSlim: "Import Slim Text",
    exportFull: "Export Full Encrypted File",
    importFull: "Import Full Encrypted File",
    exporting: "Exporting...",
    importing: "Importing...",
    loadingAccounts: "Loading accounts...",
    failedLoadAccounts: "Failed to load accounts",
    noAccountsYet: "No accounts yet",
    addFirstAccount: "Add your first Codex account to get started",
    addAccountButton: "Add Account",
    noMatching: "No matching accounts",
    tryDifferent: "Try a different account name or email address.",
    activeAccount: "Active Account",
    otherAccounts: "Other Accounts",
    sort: "Sort",
    resetEarliest: "Reset: earliest to latest",
    resetLatest: "Reset: latest to earliest",
    remainingHighest: "% remaining: highest to lowest",
    remainingLowest: "% remaining: lowest to highest",
    expiryEarliest: "Expiry: earliest to latest",
    expiryLatest: "Expiry: latest to earliest",
    searchAccounts: "Search accounts by name or email",
    clearSearch: "Clear account search",
    usageRefreshed: "Usage refreshed successfully",
    deleteConfirm: "Click delete again to confirm removal",
    forceCloseTitle: "Force close running Codex processes?",
    forceCloseDescription: "This will force close {{count}} Codex process{{suffix}} that currently block account switching.",
    afterClosing: "After closing Codex, Codex Switcher will switch to",
    afterClosingReauth: "After closing Codex, sign-in will open for",
    unsaved: "Unsaved Codex work may be lost.",
    cancel: "Cancel",
    forceClosing: "Force closing...",
    forceCloseAndSwitch: "Force close and switch account",
    forceCloseAndReauth: "Force close and sign in again",
    forceCloseRunning: "Force close running Codex processes",
    dockTitle: "Keep Codex Switcher in the Dock?",
    dockDescription: "When the window is closed, Codex Switcher can stay in the Dock or live only in the menu bar.",
    dockLater: "You can always change this later from the tray popup.",
    dontAsk: "Don't ask again",
    keepDock: "Keep in Dock",
    menuBarOnly: "Menu Bar Only",
    close: "Close",
    exportSlimTitle: "Export Slim Text",
    importSlimTitle: "Import Slim Text",
    importExistingKept: "Existing accounts are kept. Only missing accounts are imported.",
    slimSecret: "This slim string contains account secrets. Keep it private.",
    generating: "Generating...",
    exportAppears: "Export string will appear here",
    pasteConfig: "Paste config string here",
    clipboardUnavailable: "Clipboard unavailable. Please copy manually.",
    copied: "Copied",
    copyString: "Copy String",
    importMissing: "Import Missing Accounts",
    accountName: "Account Name (optional)",
    accountNamePlaceholder: "Leave blank to use email",
    chatgptLogin: "ChatGPT Login",
    importFile: "Import File",
    waitingLogin: "Waiting for browser login...",
    openLink: "Please open the following link in your browser to proceed:",
    copy: "Copy",
    open: "Open",
    oauthHost: "OAuth login must finish on the same host machine because the callback redirects to `localhost`.",
    loginHint: "Click the button below to generate a login link. You will need to open it in your browser to authenticate.",
    selectAuth: "Select auth.json file",
    browse: "Browse...",
    importAuthHint: "Import credentials from an existing Codex auth.json file",
    selectAuthError: "Please select an auth.json file",
    adding: "Adding...",
    loginLink: "Generate Login Link",
    reauthTitle: "Sign in again",
    reauthAccount: "Renew the session for {{name}}. You must sign in with the same ChatGPT account.",
    signInAgain: "Sign in again",
    identityMismatch: "This login belongs to a different ChatGPT account. Sign in with the original account.",
    reauthBlockedByCodex: "Codex is running. Close it before signing in to this account again.",
    import: "Import",
    apiKey: "API Key",
    unknown: "Unknown",
    never: "Never",
    lastUpdated: "Last updated: {{value}}",
    usageStats: "Usage Stats",
    refreshStats: "Refresh usage stats",
    usageStatsSource: "ChatGPT backend",
    statsAsOf: "Stats as of {{value}}",
    updated: "updated {{value}}",
    usageUnavailable: "Usage stats unavailable.",
    chatgptOnly: "Usage stats are available for ChatGPT accounts only.",
    lifetime: "Lifetime",
    tokens: "tokens",
    today: "Today",
    reported: "reported",
    last7: "Last 7 days",
    currentStreak: "Current streak",
    days: "days",
    peakDay: "Peak day",
    tokenActivity: "Token activity",
    last30: "Last 30 days",
    last3Months: "Last 3 months",
    last6Months: "Last 6 months",
    allReported: "All reported",
    dailyUnavailable: "Daily activity unavailable",
    moreDetails: "More usage details",
    longestTask: "Longest task",
    longestStreak: "Longest streak",
    activityInsights: "Activity insights",
    fastMode: "Fast mode",
    reasoning: "Reasoning",
    skillsExplored: "Skills explored",
    totalThreads: "Total threads",
    mostUsedPlugins: "Most used plugins",
    runs: "runs",
    fetchingUsage: "Fetching usage...",
    noRateLimit: "No rate limit data",
    fiveHourLimit: "5h Limit",
    weeklyLimit: "Weekly Limit",
    left: "left",
    resetsNow: "Resets now",
    resetsIn: "Resets in {{value}}",
    credits: "Credits",
    oneReset: "1 reset",
    manyResets: "{{count}} resets",
    noExpiry: "no expiry",
    closest: "closest {{value}}",
    clickExpiry: "Click for expiry details",
    availableResets: "Available resets",
    resetDefault: "Reset {{index}}",
    expires: "Expires {{value}}",
    localTime: "Times shown in your local time",
    session: "Session",
    weekly: "Weekly",
    usageUnavailableShort: "Usage unavailable",
    last7Short: "last 7 days",
    justNow: "just now",
    secondsAgo: "{{value}}s ago",
    minutesAgo: "{{value}}m ago",
    hoursAgo: "{{value}}h ago",
    expiryUnavailable: "Expiry unavailable",
    expired: "Expired {{value}}",
    until: "Until {{value}}",
    clickRename: "Click to rename",
    showInfo: "Show info",
    hideInfo: "Hide info",
    active: "Active",
    switching: "Switching...",
    switch: "Switch",
    codexRunning: "Codex Running",
    closeProcessesFirst: "Close all Codex processes first",
    sendingWarmup: "Sending warm-up request...",
    sendWarmup: "Send minimal warm-up request",
    autoAllAccounts: "Auto warm-up is enabled for all accounts",
    disableAutoAccount: "Disable auto warm-up for this account",
    enableAutoAccount: "Enable auto warm-up for this account",
    autoOn: "Auto: on",
    autoOff: "Auto: off",
    waitingWeeklyReset: "Waiting weekly reset",
    autoFiveHour: "Auto: 5h",
    autoWeekly: "Auto: weekly",
    refreshUsage: "Refresh usage",
    removeAccount: "Remove account",
    sessionExpired: "Session expired",
    sessionExpiredHint: "Sign in again to refresh usage and continue using this account.",
    authRefreshBlocked: "Close Codex before refreshing the active account session.",
    dock: "Dock",
    show: "Show",
    menuBar: "Menu Bar",
    openSwitcher: "Open Codex Switcher",
    quit: "Quit",
    loading: "Loading...",
    noAccounts: "No accounts configured",
    resetsNowShort: "Resets now",
    updateCheck: "Check for updates",
    updateChecking: "Checking for updates...",
    updateAvailable: "Update available: v{{version}}",
    updateLater: "Later",
    updateInstall: "Update",
    updateDownloading: "Downloading update...",
    updateReady: "Update ready. Restart to apply.",
    updateRestart: "Restart",
    updateFailed: "Update failed: {{message}}",
    updateUpToDate: "You are up to date.",
    updateDismiss: "Dismiss",
    switchedFromTray: "Switched account from tray.",
    switchFailed: "Switch failed: {{message}}",
    accountSwitchBlocked: "Account switch was blocked.",
    closeFailed: "Close failed: {{message}}",
    switchedAfterForce: "Switched account after force closing Codex.",
    switchFailedAfterForce: "Switch failed after force close: {{message}}",
    warmupSent: "Warm-up sent for {{name}}",
    warmupFailed: "Warm-up failed for {{name}}: {{message}}",
    noWarmupAccounts: "No accounts available for warm-up",
    warmupAllSent: "Warm-up sent for all {{count}} account{{suffix}}",
    warmupSummary: "Warmed {{warmed}}/{{total}}. Failed: {{failed}}",
    warmupAllFailed: "Warm-up all failed: {{message}}",
    autoWarmupSent: "Auto {{mode}} warm-up sent for {{name}}",
    autoWarmupFailed: "Auto warm-up failed for {{name}}: {{message}}",
    timedWarmupSent: "Timed warm-up sent for {{count}} account{{suffix}}",
    timedWarmupSummary: "Timed warm-up: {{warmed}} ok, {{failed}} failed",
    slimExported: "Slim text exported ({{count}} accounts).",
    slimExportFailed: "Slim export failed",
    pasteSlimFirst: "Please paste the slim text string first.",
    importedSummary: "Imported {{imported}}, skipped {{skipped}} (total {{total}})",
    slimImportFailed: "Slim import failed",
    fullExported: "Full encrypted file exported.",
    fullExportFailed: "Full export failed",
    fullImportFailed: "Full import failed",
    codexOpened: "Codex app opened.",
    openCodexFailed: "Open Codex failed: {{message}}",
  },
  zh: {
    "language.switch": "切换语言",
    "language.english": "English",
    "language.chinese": "中文",
    "process.running": "{{count}} 个 Codex 进程正在运行",
    "process.none": "0 个 Codex 进程正在运行",
    "forceClose": "强制关闭",
    "openCodex": "打开 Codex",
    "openingCodex": "正在打开…",
    "showAll": "显示所有账号名称和邮箱",
    "hideAll": "隐藏所有账号名称和邮箱",
    "refreshAll": "刷新全部用量",
    "refreshingAll": "正在刷新全部用量",
    "sendAllTraffic": "使用所有账号发送最小流量",
    "enableAutoAll": "为所有账号开启自动预热",
    "disableAutoAll": "为所有账号关闭自动预热",
    "scheduleWarmup": "为所有账号设置每日预热时间",
    "timedWarming": "正在定时预热…",
    "timedOff": "定时预热：关",
    "timedNext": "定时预热：{{time}}",
    "timedWarmup": "定时预热",
    "noTimes": "还没有添加时间。",
    add: "添加",
    remove: "移除 {{time}}",
    "switchToLight": "切换到浅色模式",
    "switchToDark": "切换到深色模式",
    "accountMenu": "账号",
    addAccount: "+ 添加账号",
    exportSlim: "导出精简文本",
    importSlim: "导入精简文本",
    exportFull: "导出完整加密文件",
    importFull: "导入完整加密文件",
    exporting: "正在导出…",
    importing: "正在导入…",
    loadingAccounts: "正在加载账号…",
    failedLoadAccounts: "加载账号失败",
    noAccountsYet: "还没有账号",
    addFirstAccount: "添加你的第一个 Codex 账号开始使用",
    addAccountButton: "添加账号",
    noMatching: "没有匹配的账号",
    tryDifferent: "请尝试其他账号名称或邮箱。",
    activeAccount: "当前账号",
    otherAccounts: "其他账号",
    sort: "排序",
    resetEarliest: "重置时间：从早到晚",
    resetLatest: "重置时间：从晚到早",
    remainingHighest: "剩余比例：从高到低",
    remainingLowest: "剩余比例：从低到高",
    expiryEarliest: "到期时间：从早到晚",
    expiryLatest: "到期时间：从晚到早",
    searchAccounts: "按名称或邮箱搜索账号",
    clearSearch: "清除账号搜索",
    usageRefreshed: "用量刷新成功",
    deleteConfirm: "再次点击删除按钮确认移除",
    forceCloseTitle: "强制关闭正在运行的 Codex 进程？",
    forceCloseDescription: "这将强制关闭当前阻止账号切换的 {{count}} 个 Codex 进程。",
    afterClosing: "关闭 Codex 后，Codex Switcher 将切换到",
    afterClosingReauth: "关闭 Codex 后，将为以下账号打开重新登录：",
    unsaved: "未保存的 Codex 工作可能会丢失。",
    cancel: "取消",
    forceClosing: "正在强制关闭…",
    forceCloseAndSwitch: "强制关闭并切换账号",
    forceCloseAndReauth: "强制关闭并重新登录",
    forceCloseRunning: "强制关闭运行中的 Codex 进程",
    dockTitle: "保持 Codex Switcher 显示在 Dock 中？",
    dockDescription: "关闭窗口后，Codex Switcher 可以继续显示在 Dock 中，或仅显示在菜单栏。",
    dockLater: "之后也可以从托盘弹窗中修改。",
    dontAsk: "不再询问",
    keepDock: "保留在 Dock 中",
    menuBarOnly: "仅菜单栏",
    close: "关闭",
    exportSlimTitle: "导出精简文本",
    importSlimTitle: "导入精简文本",
    importExistingKept: "现有账号会保留，只导入缺少的账号。",
    slimSecret: "精简字符串包含账号密钥，请妥善保管。",
    generating: "正在生成…",
    exportAppears: "导出字符串将在此显示",
    pasteConfig: "在此粘贴配置字符串",
    clipboardUnavailable: "剪贴板不可用，请手动复制。",
    copied: "已复制",
    copyString: "复制字符串",
    importMissing: "导入缺少的账号",
    accountName: "账号名称（可选）",
    accountNamePlaceholder: "留空则使用邮箱",
    chatgptLogin: "ChatGPT 登录",
    importFile: "导入文件",
    waitingLogin: "等待浏览器登录…",
    openLink: "请在浏览器中打开以下链接继续：",
    copy: "复制",
    open: "打开",
    oauthHost: "OAuth 登录必须在同一台主机上完成，因为回调地址会跳转到 `localhost`。",
    loginHint: "点击下方按钮生成登录链接，然后在浏览器中完成认证。",
    selectAuth: "选择 auth.json 文件",
    browse: "浏览…",
    importAuthHint: "从已有的 Codex auth.json 文件导入凭据",
    selectAuthError: "请选择 auth.json 文件",
    adding: "正在添加…",
    loginLink: "生成登录链接",
    reauthTitle: "重新登录",
    reauthAccount: "更新 {{name}} 的会话。必须登录同一个 ChatGPT 账号。",
    signInAgain: "重新登录",
    identityMismatch: "本次登录属于另一个 ChatGPT 账号，请使用原账号登录。",
    reauthBlockedByCodex: "Codex 正在运行，请先关闭后再重新登录此账号。",
    import: "导入",
    apiKey: "API 密钥",
    unknown: "未知",
    never: "从未",
    lastUpdated: "上次更新：{{value}}",
    usageStats: "用量统计",
    refreshStats: "刷新用量统计",
    usageStatsSource: "ChatGPT 后端",
    statsAsOf: "统计截至 {{value}}",
    updated: "更新于 {{value}}",
    usageUnavailable: "用量统计不可用。",
    chatgptOnly: "只有 ChatGPT 账号支持用量统计。",
    lifetime: "累计",
    tokens: "tokens",
    today: "今天",
    reported: "已报告",
    last7: "最近 7 天",
    currentStreak: "当前连续天数",
    days: "天",
    peakDay: "单日峰值",
    tokenActivity: "Token 活动",
    last30: "最近 30 天",
    last3Months: "最近 3 个月",
    last6Months: "最近 6 个月",
    allReported: "全部记录",
    dailyUnavailable: "每日活动不可用",
    moreDetails: "更多用量详情",
    longestTask: "最长任务",
    longestStreak: "最长连续天数",
    activityInsights: "活动分析",
    fastMode: "快速模式",
    reasoning: "推理",
    skillsExplored: "探索的技能",
    totalThreads: "线程总数",
    mostUsedPlugins: "最常用插件",
    runs: "次",
    fetchingUsage: "正在获取用量…",
    noRateLimit: "没有速率限制数据",
    fiveHourLimit: "5 小时限制",
    weeklyLimit: "每周限制",
    left: "剩余",
    resetsNow: "现在重置",
    resetsIn: "{{value}} 后重置",
    credits: "额度",
    oneReset: "1 次重置",
    manyResets: "{{count}} 次重置",
    noExpiry: "无到期时间",
    expiryUnavailable: "到期时间不可用",
    closest: "最近到期：{{value}}",
    clickExpiry: "点击查看到期详情",
    availableResets: "可用重置",
    resetDefault: "重置 {{index}}",
    expires: "到期：{{value}}",
    localTime: "时间以你的本地时间显示",
    session: "会话",
    weekly: "每周",
    usageUnavailableShort: "用量不可用",
    last7Short: "最近 7 天",
    justNow: "刚刚",
    secondsAgo: "{{value}} 秒前",
    minutesAgo: "{{value}} 分钟前",
    hoursAgo: "{{value}} 小时前",
    expired: "已于 {{value}} 到期",
    until: "有效至 {{value}}",
    clickRename: "点击重命名",
    showInfo: "显示信息",
    hideInfo: "隐藏信息",
    active: "已激活",
    switching: "正在切换…",
    switch: "切换",
    codexRunning: "Codex 运行中",
    closeProcessesFirst: "请先关闭所有 Codex 进程",
    sendingWarmup: "正在发送预热请求…",
    sendWarmup: "发送最小预热请求",
    autoAllAccounts: "已为所有账号开启自动预热",
    disableAutoAccount: "关闭此账号的自动预热",
    enableAutoAccount: "开启此账号的自动预热",
    autoOn: "自动：开",
    autoOff: "自动：关",
    waitingWeeklyReset: "等待每周重置",
    autoFiveHour: "自动：5 小时",
    autoWeekly: "自动：每周",
    refreshUsage: "刷新用量",
    removeAccount: "移除账号",
    sessionExpired: "会话已失效",
    sessionExpiredHint: "请重新登录以刷新用量并继续使用此账号。",
    authRefreshBlocked: "请先关闭 Codex，再刷新当前账号会话。",
    dock: "Dock",
    show: "显示",
    menuBar: "菜单栏",
    openSwitcher: "打开 Codex Switcher",
    quit: "退出",
    loading: "加载中…",
    noAccounts: "未配置账号",
    resetsNowShort: "现在重置",
    updateCheck: "检查更新",
    updateChecking: "正在检查更新…",
    updateAvailable: "发现新版本：v{{version}}",
    updateLater: "稍后",
    updateInstall: "更新",
    updateDownloading: "正在下载更新…",
    updateReady: "更新已准备好，重启后生效。",
    updateRestart: "重启",
    updateFailed: "更新失败：{{message}}",
    updateUpToDate: "当前已是最新版本。",
    updateDismiss: "关闭",
    switchedFromTray: "已从托盘切换账号。",
    switchFailed: "切换失败：{{message}}",
    accountSwitchBlocked: "账号切换被阻止。",
    closeFailed: "关闭失败：{{message}}",
    switchedAfterForce: "强制关闭 Codex 后已切换账号。",
    switchFailedAfterForce: "强制关闭后切换失败：{{message}}",
    warmupSent: "已为 {{name}} 发送预热请求",
    warmupFailed: "{{name}} 预热失败：{{message}}",
    noWarmupAccounts: "没有可预热的账号",
    warmupAllSent: "已为全部 {{count}} 个账号发送预热请求",
    warmupSummary: "已预热 {{warmed}}/{{total}} 个账号，失败：{{failed}}",
    warmupAllFailed: "全部预热失败：{{message}}",
    autoWarmupSent: "已为 {{name}} 发送自动{{mode}}预热请求",
    autoWarmupFailed: "{{name}} 自动预热失败：{{message}}",
    timedWarmupSent: "已定时为 {{count}} 个账号发送预热请求",
    timedWarmupSummary: "定时预热：{{warmed}} 个成功，{{failed}} 个失败",
    slimExported: "已导出精简文本（{{count}} 个账号）。",
    slimExportFailed: "精简文本导出失败",
    pasteSlimFirst: "请先粘贴精简文本字符串。",
    importedSummary: "已导入 {{imported}} 个，跳过 {{skipped}} 个（共 {{total}} 个）",
    slimImportFailed: "精简文本导入失败",
    fullExported: "已导出完整加密文件。",
    fullExportFailed: "完整文件导出失败",
    fullImportFailed: "完整文件导入失败",
    codexOpened: "Codex 应用已打开。",
    openCodexFailed: "打开 Codex 失败：{{message}}",
  },
};

function normalizeLanguage(value: unknown): Language | null {
  return value === "zh" || value === "en" ? value : null;
}

function getInitialLanguage(): Language {
  if (typeof window !== "undefined") {
    try {
      const stored = normalizeLanguage(window.localStorage.getItem(LANGUAGE_STORAGE_KEY));
      if (stored) return stored;
    } catch {
      // Ignore storage errors and use the browser language below.
    }

    if (window.navigator.language.toLowerCase().startsWith("zh")) return "zh";
  }

  return "en";
}

function interpolate(template: string, values?: Record<string, string | number>): string {
  if (!values) return template;
  return template.replace(/\{\{(\w+)\}\}/g, (_, key: string) => String(values[key] ?? ""));
}

interface LanguageContextValue {
  language: Language;
  locale: AppLocale;
  setLanguage: (language: Language) => void;
  t: (key: string, values?: Record<string, string | number>) => string;
}

const LanguageContext = createContext<LanguageContextValue | null>(null);

export function LanguageProvider({ children }: { children: ReactNode }) {
  const [language, setLanguageState] = useState<Language>(getInitialLanguage);

  const setLanguage = useCallback((next: Language) => {
    setLanguageState(next);
    try {
      window.localStorage.setItem(LANGUAGE_STORAGE_KEY, next);
    } catch {
      // Ignore storage errors; the current window still changes language.
    }

    if (isTauriRuntime()) {
      void import("@tauri-apps/api/event")
        .then(({ emit }) => emit(LANGUAGE_CHANGED_EVENT, next))
        .catch(() => {});
    }
  }, []);

  useEffect(() => {
    document.documentElement.lang = language === "zh" ? "zh-CN" : "en";

    const handleStorage = (event: StorageEvent) => {
      if (event.key !== LANGUAGE_STORAGE_KEY) return;
      const next = normalizeLanguage(event.newValue);
      if (next) setLanguageState(next);
    };

    window.addEventListener("storage", handleStorage);
    let unlisten: (() => void) | undefined;

    if (isTauriRuntime()) {
      void import("@tauri-apps/api/event").then(async ({ listen }) => {
        unlisten = await listen<Language>(LANGUAGE_CHANGED_EVENT, ({ payload }) => {
          const next = normalizeLanguage(payload);
          if (next) setLanguageState(next);
        });
      });
    }

    return () => {
      window.removeEventListener("storage", handleStorage);
      unlisten?.();
    };
  }, [language]);

  const value = useMemo<LanguageContextValue>(
    () => ({
      language,
      locale: language === "zh" ? "zh-CN" : "en-US",
      setLanguage,
      t: (key, values) => interpolate(messages[language][key] ?? messages.en[key] ?? key, values),
    }),
    [language, setLanguage]
  );

  return <LanguageContext.Provider value={value}>{children}</LanguageContext.Provider>;
}

export function useI18n(): LanguageContextValue {
  const context = useContext(LanguageContext);
  if (!context) {
    throw new Error("useI18n must be used inside LanguageProvider");
  }
  return context;
}

export function LanguageToggle({ compact = false }: { compact?: boolean }) {
  const { language, setLanguage, t } = useI18n();
  const nextLanguage = language === "zh" ? "en" : "zh";

  return (
    <button
      type="button"
      onClick={() => setLanguage(nextLanguage)}
      className={`flex items-center justify-center rounded-lg bg-gray-100 text-xs font-semibold text-gray-700 transition-colors hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700 shrink-0 ${compact ? "h-8 px-2" : "h-10 px-3"}`}
      title={t("language.switch")}
      aria-label={t("language.switch")}
    >
      {language === "zh" ? "中文 / EN" : "EN / 中文"}
    </button>
  );
}
