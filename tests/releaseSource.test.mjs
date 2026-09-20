import assert from "node:assert/strict";
import test from "node:test";
import fs from "node:fs";
import path from "node:path";
import { parseReleaseTag, verifyReleaseSource } from "../scripts/verify-release-source.mjs";

const root = path.resolve(import.meta.dirname, "..");
const embeddedVersion = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8")).version;
const releaseTag = `v${embeddedVersion}`;

test("release source verification accepts a matching immutable tag and embedded versions", () => {
  const gitCalls = [];
  const result = verifyReleaseSource({
    root,
    releaseTag,
    runGit: (args) => {
      gitCalls.push(args);
      return "release-commit";
    },
  });

  assert.deepEqual(gitCalls, [
    ["rev-parse", "HEAD"],
    ["rev-parse", "--verify", `refs/tags/${releaseTag}^{commit}`],
  ]);
  assert.deepEqual(result, {
    checkedOutCommit: "release-commit",
    releaseCommit: "release-commit",
    releaseTag,
    expectedVersion: embeddedVersion,
  });
});

test("release source verification rejects a branch-like or mismatched ref before a build", () => {
  assert.throws(() => parseReleaseTag("main"), /existing semantic-version tag/);
  assert.throws(
    () => verifyReleaseSource({ root, releaseTag: "v999.999.999", runGit: () => "release-commit" }),
    /Embedded release version 999\.999\.999 does not match/,
  );
});
