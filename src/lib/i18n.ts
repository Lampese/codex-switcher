export type SupportedLanguage = "en-US" | "zh-CN";

const messages = {
  "en-US": {
    settingsTitle: "Settings",
  },
  "zh-CN": {
    settingsTitle: "设置",
  },
} as const;

export function resolveInitialLanguage(language?: string): SupportedLanguage {
  return language?.toLowerCase().startsWith("zh") ? "zh-CN" : "en-US";
}

export function translate(
  key: keyof (typeof messages)["en-US"],
  language = resolveInitialLanguage(
    typeof navigator === "undefined" ? undefined : navigator.language
  )
): string {
  return messages[language][key] ?? messages["en-US"][key];
}
