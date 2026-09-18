import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

export const RELEASE_TAG_PATTERN = /^v\d+\.\d+\.\d+$/;

export const parseReleaseTag = (input) => {
  const releaseTag = input?.trim();
  if (!releaseTag || !RELEASE_TAG_PATTERN.test(releaseTag)) {
    throw new Error(`Release ref must be an existing semantic-version tag, received: ${releaseTag ?? ""}`);
  }
  return releaseTag;
};

const readVersion = (root, file) => {
  const contents = fs.readFileSync(path.join(root, file), "utf8");
  if (file.endsWith(".json")) return JSON.parse(contents).version;
  if (file.endsWith("Cargo.toml")) return contents.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  return contents.match(/\[\[package\]\]\r?\nname = "codex-switcher"\r?\nversion = "([^"]+)"/)?.[1];
};

export const verifyReleaseSource = ({ root = process.cwd(), releaseTag: input, runGit } = {}) => {
  const releaseTag = parseReleaseTag(input);
  const git = runGit ?? ((args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim());
  const tagRef = `refs/tags/${releaseTag}`;
  const checkedOutCommit = git(["rev-parse", "HEAD"]);
  let releaseCommit;

  try {
    releaseCommit = git(["rev-parse", "--verify", `${tagRef}^{commit}`]);
  } catch {
    throw new Error(`Release tag ${releaseTag} is not available in the checked-out repository`);
  }

  if (checkedOutCommit !== releaseCommit) {
    throw new Error(`Checked-out source ${checkedOutCommit} does not match ${releaseTag} commit ${releaseCommit}`);
  }

  const expectedVersion = releaseTag.slice(1);
  const versionFiles = [
    "package.json",
    "src-tauri/tauri.conf.json",
    "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock",
  ];

  const mismatches = versionFiles
    .map((file) => [file, readVersion(root, file)])
    .filter(([, version]) => version !== expectedVersion);

  if (mismatches.length > 0) {
    const details = mismatches.map(([file, version]) => `${file}=${version ?? "<missing>"}`).join(", ");
    throw new Error(`Embedded release version ${expectedVersion} does not match: ${details}`);
  }

  return { checkedOutCommit, releaseCommit, releaseTag, expectedVersion };
};

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  const result = verifyReleaseSource({ releaseTag: process.argv[2] });
  console.log(`Release source verified: ${result.releaseTag} at ${result.checkedOutCommit}`);
}
