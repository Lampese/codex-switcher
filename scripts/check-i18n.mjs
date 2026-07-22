import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const strict = process.argv.includes("--strict");
const manifest = JSON.parse(readFileSync(join(root, "locales", "manifest.json"), "utf8"));
const localeCodes = manifest.map(({ code }) => code);
if (!localeCodes.includes("en-US")) throw new Error("manifest must include the en-US fallback locale");
if (new Set(localeCodes).size !== localeCodes.length) throw new Error("manifest locale codes must be unique");

function flatten(value, prefix = "") {
  return Object.entries(value).flatMap(([key, child]) => {
    const path = prefix ? `${prefix}.${key}` : key;
    return child && typeof child === "object" && !Array.isArray(child)
      ? flatten(child, path)
      : [path];
  });
}

const referenceKeys = new Map();
const catalogWarnings = [];

for (const catalog of ["ui.json", "native.json"]) {
  const reference = new Set(
    flatten(JSON.parse(readFileSync(join(root, "locales", "en-US", catalog), "utf8")))
  );
  referenceKeys.set(catalog, reference);

  for (const { code } of manifest) {
    const keys = new Set(
      flatten(JSON.parse(readFileSync(join(root, "locales", code, catalog), "utf8")))
    );
    const missing = [...reference].filter((key) => !keys.has(key));
    const extra = [...keys].filter((key) => !reference.has(key));
    if (code !== "en-US" && (missing.length || extra.length)) {
      catalogWarnings.push({ code, catalog, missing, extra });
    }
  }
}

if (strict && catalogWarnings.length) {
  throw new Error(
    catalogWarnings
      .map(({ code, catalog, missing, extra }) =>
        `${code}/${catalog} key mismatch\nMissing: ${missing.join(", ") || "none"}\nExtra: ${extra.join(", ") || "none"}`
      )
      .join("\n\n")
  );
}

if (!strict) {
  for (const { code, catalog, missing, extra } of catalogWarnings) {
    console.warn(
      `\x1b[33m⚠ ${code}/${catalog} is incomplete; missing text falls back to en-US` +
        ` / 翻译尚未完整，缺失文案将回退英文` +
        `\n  Missing / 缺少: ${missing.join(", ") || "none"}` +
        `\n  Extra / 多余: ${extra.join(", ") || "none"}\x1b[0m`
    );
  }
}

function sourceFiles(directory, extensions) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path, extensions);
    return extensions.some((extension) => entry.name.endsWith(extension)) ? [path] : [];
  });
}

function assertUsedKeysExist(files, catalog) {
  const known = referenceKeys.get(catalog);
  const missing = [];
  const keyPattern = /(?:\bt|\bi18n\.t|\bappI18n\.t)\(\s*["']([^"']+)["']/g;

  for (const file of files) {
    const contents = readFileSync(file, "utf8");
    for (const match of contents.matchAll(keyPattern)) {
      if (!known.has(match[1])) missing.push(`${file.slice(root.length + 1)}: ${match[1]}`);
    }
  }

  if (missing.length) {
    throw new Error(`Unknown keys used with ${catalog}:\n${missing.join("\n")}`);
  }
}

assertUsedKeysExist(sourceFiles(join(root, "src"), [".ts", ".tsx"]), "ui.json");
assertUsedKeysExist(
  [join(root, "src-tauri", "src", "app_menu.rs"), join(root, "src-tauri", "src", "tray.rs")],
  "native.json"
);

const hardcodedToastFiles = [];
for (const file of sourceFiles(join(root, "src"), [".ts", ".tsx"])) {
  const contents = readFileSync(file, "utf8");
  if (/showWarmupToast\(\s*[`"']/.test(contents)) {
    hardcodedToastFiles.push(file.slice(root.length + 1));
  }
}

if (strict && hardcodedToastFiles.length) {
  throw new Error(
    `Non-localized toast literals found:\n${hardcodedToastFiles.join("\n")}`
  );
}

if (!strict && hardcodedToastFiles.length) {
  console.warn(
    `\x1b[33m⚠ Non-localized toast literals found; build will continue` +
      ` / 发现未国际化的 Toast，构建将继续` +
      `\n${hardcodedToastFiles.map((file) => `  ${file}`).join("\n")}\x1b[0m`
  );
}

console.log(
  `\x1b[32m✓ i18n ${strict ? "strict " : ""}check passed for ${manifest.length} locales` +
    ` / 国际化资源${strict ? "严格" : "常规"}检查通过，共 ${manifest.length} 种语言\x1b[0m`
);
