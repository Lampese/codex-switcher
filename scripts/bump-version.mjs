import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const input = process.argv[2];

if (!input) {
  console.error(
    "Usage: node scripts/bump-version.mjs <version|major|minor|patch>",
  );
  process.exit(1);
}

const isBumpType = ["major", "minor", "patch"].includes(input);

const readFile = (relativePath) =>
  fs.readFileSync(path.join(root, relativePath), "utf8");

const writeFile = (relativePath, contents) =>
  fs.writeFileSync(path.join(root, relativePath), contents, "utf8");

const parseVersion = (version) => {
  const match = version.match(/^(\d+)\.(\d+)\.(\d+)$/);
  if (!match) return null;
  return {
    major: Number(match[1]),
    minor: Number(match[2]),
    patch: Number(match[3]),
  };
};

const formatVersion = ({ major, minor, patch }) =>
  `${major}.${minor}.${patch}`;

const bumpVersion = (version, bumpType) => {
  const parsed = parseVersion(version);
  if (!parsed) return null;
  if (bumpType === "major") {
    return formatVersion({
      major: parsed.major + 1,
      minor: 0,
      patch: 0,
    });
  }
  if (bumpType === "minor") {
    return formatVersion({
      major: parsed.major,
      minor: parsed.minor + 1,
      patch: 0,
    });
  }
  return formatVersion({
    major: parsed.major,
    minor: parsed.minor,
    patch: parsed.patch + 1,
  });
};

const packageJson = readFile("package.json");
const currentVersionMatch = packageJson.match(
  /"version"\s*:\s*"(\d+\.\d+\.\d+)"/,
);

if (!currentVersionMatch) {
  console.error("Could not find version in package.json");
  process.exit(1);
}

const currentVersion = currentVersionMatch[1];
const nextVersion = isBumpType
  ? bumpVersion(currentVersion, input)
  : input;

if (!nextVersion || !parseVersion(nextVersion)) {
  console.error(`Invalid version: ${input}`);
  process.exit(1);
}

const replaceJsonVersion = (contents) =>
  contents.replace(
    /"version"\s*:\s*"\d+\.\d+\.\d+"/,
    `"version": "${nextVersion}"`,
  );

const replaceCargoVersion = (contents) =>
  contents.replace(
    /^version\s*=\s*"\d+\.\d+\.\d+"/m,
    `version = "${nextVersion}"`,
  );

const replaceCargoLockVersion = (contents) => {
  const blockRegex =
    /(\[\[package\]\]\r?\nname = "codex-switcher"\r?\nversion = ")\d+\.\d+\.\d+("[\s\S]*?\r?\n\r?\n)/;
  if (!blockRegex.test(contents)) {
    console.error(
      "Could not find codex-switcher package in Cargo.lock",
    );
    process.exit(1);
  }
  return contents.replace(blockRegex, `$1${nextVersion}$2`);
};

writeFile("package.json", replaceJsonVersion(packageJson));
writeFile(
  "src-tauri/tauri.conf.json",
  replaceJsonVersion(readFile("src-tauri/tauri.conf.json")),
);
writeFile(
  "src-tauri/Cargo.toml",
  replaceCargoVersion(readFile("src-tauri/Cargo.toml")),
);
writeFile(
  "src-tauri/Cargo.lock",
  replaceCargoLockVersion(readFile("src-tauri/Cargo.lock")),
);

console.log(`Version bumped: ${currentVersion} -> ${nextVersion}`);
