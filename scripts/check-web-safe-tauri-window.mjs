import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const appSource = readFileSync(join(root, "src", "App.tsx"), "utf8");

const unsafeStaticWindowImport =
  /import\s*\{[^}]*\bgetCurrentWindow\b[^}]*\}\s*from\s*["']@tauri-apps\/api\/window["']/.test(
    appSource
  );
const unsafeTopLevelWindowHandle =
  /const\s+\w+\s*=\s*getCurrentWindow\s*\(/.test(appSource);

if (unsafeStaticWindowImport || unsafeTopLevelWindowHandle) {
  throw new Error(
    "App must not create a Tauri window handle at module load; the web dashboard lacks __TAURI_INTERNALS__."
  );
}
