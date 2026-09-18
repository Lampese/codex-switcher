import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
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
    Close: "Close", Cancel: "Cancel", Add: "Add", Show: "Show", "Menu Bar": "Menu Bar",
    Minimize: "Minimize", Restore: "Restore", Maximize: "Maximize", "Open Codex app": "Open Codex app",
    "Close running Codex processes": "Close running Codex processes", "Codex running": "Codex running",
    "0 Codex running": "0 Codex running", "Refresh usage stats": "Refresh usage stats",
    "Refreshing all usage": "Refreshing all usage",
    "Usage refreshed successfully": "Usage refreshed successfully", "Loading display settings...": "Loading display settings...",
    "Could not update display settings": "Could not update display settings", "Last updated": "Last updated",
    "Active": "Active", "Sending warm-up request...": "Sending warm-up request...",
    "Send minimal warm-up request": "Send minimal warm-up request", "Enable auto warm-up for this account": "Enable auto warm-up for this account",
    "Disable auto warm-up for this account": "Disable auto warm-up for this account", "Auto warm-up is enabled for all accounts": "Auto warm-up is enabled for all accounts",
    "Show all account names and emails": "Show all account names and emails", "Hide all account names and emails": "Hide all account names and emails",
    "Click delete again to confirm removal": "Click delete again to confirm removal", "More usage details": "More usage details",
    "Activity insights": "Activity insights", "Most used plugins": "Most used plugins", runs: "runs",
    "Daily activity unavailable": "Daily activity unavailable", "Token activity range": "Token activity range", "ChatGPT backend": "ChatGPT backend",
    "ChatGPT-only usage stats": "Usage stats are available for ChatGPT accounts only.", "Stats as of {{date}}": "Stats as of {{date}}",
    "updated {{value}}": "updated {{value}}", "Expired {{date}}": "Expired {{date}}", "Until {{date}}": "Until {{date}}",
    "Last updated: {{value}}": "Last updated: {{value}}", "warmupSentFor": "Warm-up sent for {{account}}",
    "{{count}}s ago": "{{count}}s ago", "{{count}}m ago": "{{count}}m ago", "{{count}}h ago": "{{count}}h ago",
    on: "on", off: "off", "Close running Codex processes and switch account": "Close running Codex processes and switch account",
    "warmupFailedFor": "Warm-up failed for {{account}}: {{error}}", "warmupAllSent": "Warm-up sent for all {{count}} accounts",
    "warmupAllSummary": "Warmed {{warmed}}/{{total}}. Failed: {{failed}}", "warmupAllFailed": "Warm-up all failed: {{error}}",
    "autoWarmupSentFor": "Auto {{mode}} warm-up sent for {{account}}", "autoWarmupFailedFor": "Auto warm-up failed for {{account}}: {{error}}",
    "timedWarmupSent": "Timed warm-up sent for {{count}} accounts", "timedWarmupSummary": "Timed warm-up: {{warmed}} ok, {{failed}} failed",
    "At least one display icon": "At least one of the Dock or tray icons stays visible so you can reopen Codex Switcher.",
    "Account Name (optional)": "Account Name (optional)", "Leave blank to use email": "Leave blank to use email",
    "Open the following link in your browser to proceed:": "Please open the following link in your browser to proceed:",
    "Copied!": "Copied!", Copy: "Copy", Open: "Open", "Click below to generate a login link. Open it in your browser to authenticate.": "Click the button below to generate a login link. You will need to open it in your browser to authenticate.",
    "Select auth.json file": "Select auth.json file", "Browse...": "Browse...", "Import credentials from an existing Codex auth.json file": "Import credentials from an existing Codex auth.json file", "Adding...": "Adding...",
    "Update available: v{{version}}": "Update available: v{{version}}", Later: "Later", Update: "Update", "Downloading update...": "Downloading update...", "Update ready. Restart to apply.": "Update ready. Restart to apply.", Restart: "Restart", "Update failed": "Update failed", Dismiss: "Dismiss",
    limit: "limit", left: "left", "resets {{value}}": "resets {{value}}", "Resets in {{value}}": "Resets in {{value}}", "Fetching usage...": "Fetching usage...", "No rate limit data": "No rate limit data", "5h Limit": "5h Limit", "Weekly Limit": "Weekly Limit", "Credits: {{value}}": "Credits: {{value}}",
    Auto: "Auto", "Disable auto warm-up for all accounts": "Disable auto warm-up for all accounts", "Enable auto warm-up for all accounts": "Enable auto warm-up for all accounts", Dock: "Dock",
    "Warming...": "Warming...", "Auto warming...": "Auto warming...", "Waiting {{value}}": "Waiting {{value}}", "Waiting reset": "Waiting reset",
    Timed: "Timed", "Timed warming...": "Timed warming...", "No times added yet.": "No times added yet.", "No accounts yet": "No accounts yet", "Add your first Codex account to get started": "Add your first Codex account to get started", "Active Account": "Active Account", "Other Accounts": "Other Accounts", Sort: "Sort", "Reset: earliest to latest": "Reset: earliest to latest", "Reset: latest to earliest": "Reset: latest to earliest", "% remaining: highest to lowest": "% remaining: highest to lowest", "% remaining: lowest to highest": "% remaining: lowest to highest", "Expiry: earliest to latest": "Expiry: earliest to latest", "Expiry: latest to earliest": "Expiry: latest to earliest", "Hide account search": "Hide account search", "Search accounts": "Search accounts", "Search accounts by name or email": "Search accounts by name or email", "Clear account search": "Clear account search",
    "Close running Codex processes?": "Close running Codex processes?", closeProcessPrompt: "This will {{action}} {{count}} Codex process(es) that currently block account switching.", "force close": "force close", "gracefully close": "gracefully close", "Codex close preference summary": "Codex will {{action}}. You can change this in Settings.", "be force closed": "be force closed", "Close and switch account": "Close and switch account", "Close Codex": "Close Codex", "Force close Codex": "Force close Codex", "Remember this selection": "Remember this selection", "Stops Codex immediately. Unsaved work may be lost.": "Stops Codex immediately. Unsaved work may be lost.", "Asks Codex to quit normally so it can finish cleanup.": "Asks Codex to quit normally so it can finish cleanup.", "After closing Codex, Codex Switcher will switch to {{account}}.": "After closing Codex, Codex Switcher will switch to {{account}}.", "Checking for a desktop app to reopen...": "Checking for a desktop app to reopen...", "Codex desktop will reopen automatically.": "Codex desktop will reopen automatically.", "Codex desktop will stay closed.": "Codex desktop will stay closed.", "You can change this in Settings.": "You can change this in Settings.", "Reopen Codex desktop after close": "Reopen Codex desktop after close", "You can change this later in Settings. Terminal and IDE sessions will not reopen.": "You can change this later in Settings. Terminal and IDE sessions will not reopen.", "No supported desktop app could be identified for reopening. Codex will only be closed.": "No supported desktop app could be identified for reopening. Codex will only be closed.", "Unsaved Codex work may be lost.": "Unsaved Codex work may be lost.", "Force closing...": "Force closing...", "Closing...": "Closing...", "Keep Codex Switcher in the Dock?": "Keep Codex Switcher in the Dock?", "When the window is closed, Codex Switcher can stay in the Dock or live only in the menu bar.": "When the window is closed, Codex Switcher can stay in the Dock or live only in the menu bar.", "You can always change this later from the tray popup.": "You can always change this later from the tray popup.", "Don't ask again": "Don't ask again", "Keep in Dock": "Keep in Dock", "Existing accounts are kept. Only missing accounts are imported.": "Existing accounts are kept. Only missing accounts are imported.", "This slim string contains account secrets. Keep it private.": "This slim string contains account secrets. Keep it private.", "Generating...": "Generating...", "Export string will appear here": "Export string will appear here", "Paste config string here": "Paste config string here", Copied: "Copied", "Copy String": "Copy String", "Import Missing Accounts": "Import Missing Accounts",
    "Could not check running Codex processes. Try again.": "Could not check running Codex processes. Try again.", "Switched account from tray.": "Switched account from tray.", "Account switch was blocked.": "Account switch was blocked.", "No accounts available for warm-up": "No accounts available for warm-up", "Slim text exported ({{count}} accounts).": "Slim text exported ({{count}} accounts).", "Slim export failed": "Slim export failed", "Please paste the slim text string first.": "Please paste the slim text string first.", "Slim import failed": "Slim import failed", "Full encrypted file exported.": "Full encrypted file exported.", "Full export failed": "Full export failed", "Full import failed": "Full import failed", "Codex app opened.": "Codex app opened.",
    "Graceful close description": "Graceful close lets Codex finish cleanup. Force close stops it immediately and may lose unsaved work.",
    "Reopen preference description": "Applies to detected Codex desktop apps on macOS and Windows. When switching accounts, the app reopens after the switch succeeds.",
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
    Close: "关闭", Cancel: "取消", Add: "添加", Show: "显示", "Menu Bar": "菜单栏", Minimize: "最小化",
    Restore: "还原", Maximize: "最大化", "Open Codex app": "打开 Codex 应用", "Close running Codex processes": "关闭正在运行的 Codex 进程",
    "Codex running": "Codex 运行中", "0 Codex running": "0 个 Codex 运行中", "Refresh usage stats": "刷新用量统计",
    "Refreshing all usage": "正在刷新全部用量", "Loading display settings...": "正在加载显示设置……",
    "Could not update display settings": "无法更新显示设置", "Last updated": "上次更新", Active: "当前账号",
    "Sending warm-up request...": "正在发送预热请求……", "Send minimal warm-up request": "发送最小预热请求",
    "Enable auto warm-up for this account": "为此账号启用自动预热", "Disable auto warm-up for this account": "为此账号停用自动预热",
    "Auto warm-up is enabled for all accounts": "已为所有账号启用自动预热", "Show all account names and emails": "显示所有账号名称和邮箱",
    "Hide all account names and emails": "隐藏所有账号名称和邮箱", "Click delete again to confirm removal": "再次点击删除以确认移除",
    "More usage details": "更多用量详情", "Activity insights": "活动分析", "Most used plugins": "最常用插件", runs: "次",
    "Daily activity unavailable": "每日活动不可用", "Token activity range": "令牌活动范围", "ChatGPT backend": "ChatGPT 后端",
    "ChatGPT-only usage stats": "仅 ChatGPT 账号提供用量统计。", "Stats as of {{date}}": "统计截至 {{date}}", "updated {{value}}": "更新于 {{value}}",
    "Expired {{date}}": "已于 {{date}} 过期", "Until {{date}}": "有效至 {{date}}", "Last updated: {{value}}": "上次更新：{{value}}",
    "{{count}}s ago": "{{count}} 秒前", "{{count}}m ago": "{{count}} 分钟前", "{{count}}h ago": "{{count}} 小时前", on: "开", off: "关",
    "Close running Codex processes and switch account": "关闭正在运行的 Codex 进程并切换账号",
    "warmupSentFor": "已为 {{account}} 发送预热请求", "warmupFailedFor": "{{account}} 预热失败：{{error}}",
    "warmupAllSent": "已为全部 {{count}} 个账号发送预热请求", "warmupAllSummary": "已预热 {{warmed}}/{{total}} 个账号，失败：{{failed}}",
    "warmupAllFailed": "全部预热失败：{{error}}", "autoWarmupSentFor": "已为 {{account}} 发送 {{mode}} 自动预热",
    "autoWarmupFailedFor": "{{account}} 自动预热失败：{{error}}", "timedWarmupSent": "已为 {{count}} 个账号发送定时预热",
    "timedWarmupSummary": "定时预热：成功 {{warmed}}，失败 {{failed}}",
    "At least one display icon": "Dock 或托盘图标至少保留一个，以便重新打开 Codex Switcher。",
    "Account Name (optional)": "账号名称（可选）", "Leave blank to use email": "留空则使用邮箱",
    "Open the following link in your browser to proceed:": "请在浏览器中打开以下链接以继续：", "Copied!": "已复制！", Copy: "复制", Open: "打开",
    "Click below to generate a login link. Open it in your browser to authenticate.": "点击下方按钮生成登录链接，然后在浏览器中打开并完成认证。",
    "Select auth.json file": "选择 auth.json 文件", "Browse...": "浏览……", "Import credentials from an existing Codex auth.json file": "从现有 Codex auth.json 文件导入凭据", "Adding...": "正在添加……",
    "Update available: v{{version}}": "有可用更新：v{{version}}", Later: "稍后", Update: "更新", "Downloading update...": "正在下载更新……", "Update ready. Restart to apply.": "更新已准备好，重启后生效。", Restart: "重启", "Update failed": "更新失败", Dismiss: "忽略",
    limit: "限额", left: "剩余", "resets {{value}}": "{{value}} 后重置", "Resets in {{value}}": "{{value}} 后重置", "Fetching usage...": "正在获取用量……", "No rate limit data": "没有速率限制数据", "5h Limit": "5 小时限额", "Weekly Limit": "每周限额", "Credits: {{value}}": "额度：{{value}}",
    Auto: "自动", "Disable auto warm-up for all accounts": "停用全部账号自动预热", "Enable auto warm-up for all accounts": "启用全部账号自动预热", Dock: "Dock",
    "Warming...": "正在预热……", "Auto warming...": "正在自动预热……", "Waiting {{value}}": "等待 {{value}}", "Waiting reset": "等待重置",
    Timed: "定时", "Timed warming...": "正在定时预热……", "No times added yet.": "尚未添加时间。", "No accounts yet": "暂无账号", "Add your first Codex account to get started": "添加第一个 Codex 账号以开始使用", "Active Account": "当前账号", "Other Accounts": "其他账号", Sort: "排序", "Reset: earliest to latest": "重置：从最早到最晚", "Reset: latest to earliest": "重置：从最晚到最早", "% remaining: highest to lowest": "剩余百分比：从高到低", "% remaining: lowest to highest": "剩余百分比：从低到高", "Expiry: earliest to latest": "过期时间：从早到晚", "Expiry: latest to earliest": "过期时间：从晚到早", "Hide account search": "隐藏账号搜索", "Search accounts": "搜索账号", "Search accounts by name or email": "按名称或邮箱搜索账号", "Clear account search": "清除账号搜索",
    "Close running Codex processes?": "关闭正在运行的 Codex 进程？", closeProcessPrompt: "{{action}} {{count}} 个当前阻止账号切换的 Codex 进程。", "force close": "强制关闭", "gracefully close": "正常关闭", "Codex close preference summary": "Codex 将{{action}}。你可以在设置中更改。", "be force closed": "被强制关闭", "Close and switch account": "关闭并切换账号", "Close Codex": "关闭 Codex", "Force close Codex": "强制关闭 Codex", "Remember this selection": "记住此选择", "Stops Codex immediately. Unsaved work may be lost.": "立即停止 Codex，未保存的工作可能会丢失。", "Asks Codex to quit normally so it can finish cleanup.": "请求 Codex 正常退出，以便完成清理。", "After closing Codex, Codex Switcher will switch to {{account}}.": "关闭 Codex 后，Codex Switcher 将切换到 {{account}}。", "Checking for a desktop app to reopen...": "正在检查可重新打开的桌面应用……", "Codex desktop will reopen automatically.": "Codex 桌面应用将自动重新打开。", "Codex desktop will stay closed.": "Codex 桌面应用将保持关闭。", "You can change this in Settings.": "你可以在设置中更改。", "Reopen Codex desktop after close": "关闭后重新打开 Codex 桌面应用", "You can change this later in Settings. Terminal and IDE sessions will not reopen.": "你可以稍后在设置中更改。终端和 IDE 会话不会重新打开。", "No supported desktop app could be identified for reopening. Codex will only be closed.": "无法识别可重新打开的受支持桌面应用，只会关闭 Codex。", "Unsaved Codex work may be lost.": "未保存的 Codex 工作可能会丢失。", "Force closing...": "正在强制关闭……", "Closing...": "正在关闭……", "Keep Codex Switcher in the Dock?": "将 Codex Switcher 保留在 Dock 中？", "When the window is closed, Codex Switcher can stay in the Dock or live only in the menu bar.": "关闭窗口后，Codex Switcher 可以保留在 Dock 中，也可以只显示在菜单栏。", "You can always change this later from the tray popup.": "你始终可以稍后从托盘弹窗更改。", "Don't ask again": "不再询问", "Keep in Dock": "保留在 Dock 中", "Existing accounts are kept. Only missing accounts are imported.": "保留现有账号，只导入缺失账号。", "This slim string contains account secrets. Keep it private.": "此精简字符串包含账号密钥，请妥善保密。", "Generating...": "正在生成……", "Export string will appear here": "导出字符串将在此显示", "Paste config string here": "在此粘贴配置字符串", Copied: "已复制", "Copy String": "复制字符串", "Import Missing Accounts": "导入缺失账号",
    "Could not check running Codex processes. Try again.": "无法检查正在运行的 Codex 进程，请重试。", "Switched account from tray.": "已从托盘切换账号。", "Account switch was blocked.": "账号切换被阻止。", "No accounts available for warm-up": "没有可预热的账号", "Slim text exported ({{count}} accounts).": "已导出精简文本（{{count}} 个账号）。", "Slim export failed": "精简导出失败", "Please paste the slim text string first.": "请先粘贴精简文本字符串。", "Slim import failed": "精简导入失败", "Full encrypted file exported.": "已导出完整加密文件。", "Full export failed": "完整导出失败", "Full import failed": "完整导入失败", "Codex app opened.": "Codex 应用已打开。",
    "Graceful close description": "正常关闭会让 Codex 完成清理；强制关闭会立即停止进程，可能丢失未保存的工作。",
    "Reopen preference description": "适用于 macOS 和 Windows 上检测到的 Codex 桌面应用。切换账号成功后，应用会重新打开。",
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
  t: (key: string, variables?: Record<string, string | number>) => string;
};

const LanguageContext = createContext<LanguageContextValue | null>(null);

export function resolveInitialLanguage(language?: string): SupportedLanguage {
  const normalized = language?.toLowerCase();
  return normalized === "zh-cn" || normalized === "zh-hans" || normalized === "zh-sg"
    ? "zh-CN"
    : "en-US";
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

export function I18nProvider({ children }: { children: ReactNode }) {
  const [language, setLanguageState] = useState<SupportedLanguage>(getInitialLanguage);

  useEffect(() => {
    void i18n.changeLanguage(language);
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
  const value = useMemo(
    () => ({
      language,
      setLanguage,
      t: (key: string, variables: Record<string, string | number> = {}) =>
        translateMessage(key, variables, language),
    }),
    [language, setLanguage]
  );

  return createElement(LanguageContext.Provider, { value }, children);
}

export function useI18n(): LanguageContextValue {
  const context = useContext(LanguageContext);
  if (!context) throw new Error("useI18n must be used inside I18nProvider");
  return context;
}
