import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

test("warm-up policy persistence is explicit while projection loading stays read-only", () => {
  assert.equal(
    appSource.match(/invokeBackend\("set_warmup_policy"/g)?.length ?? 0,
    1,
    "App should have one explicit persistence boundary"
  );
  assert.doesNotMatch(appSource, /warmupStateLoaded/);
  assert.match(appSource, /invokeBackend\("set_warmup_policy", overrides\)/);
  assert.doesNotMatch(appSource, /auto_warmup_account_ids: Array\.from\(autoWarmupAccountIds\)/);
  assert.match(appSource, /persistWarmupPolicy\(\{ auto_warmup_all_enabled: next \}\)/);
  assert.match(appSource, /persistWarmupPolicy\(\{ auto_warmup_account_ids: Array\.from\(next\) \}\)/);
  assert.match(appSource, /persistWarmupPolicy\(\{ timed_warmup_enabled: next \}\)/);
  assert.match(appSource, /persistWarmupPolicy\(\{ timed_warmup_times: next \}\)/);

  const traySource = readFileSync(new URL("../src/TrayMenu.tsx", import.meta.url), "utf8");
  assert.match(traySource, /invokeBackend\("set_warmup_policy", \{\s*auto_warmup_all_enabled: next,?\s*\}\)/);
  assert.doesNotMatch(traySource, /\.\.\.state\.policy/);
});
