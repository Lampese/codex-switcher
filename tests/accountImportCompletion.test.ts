import assert from "node:assert/strict";
import test from "node:test";
import { completeImportWithoutBlockingOnUsage } from "../src/lib/accountImportCompletion.ts";

test("file import completes when usage remains pending", async () => {
  const account = { id: "imported" };
  const steps: string[] = [];
  const result = await completeImportWithoutBlockingOnUsage(
    async () => { steps.push("saved"); return account; },
    async () => { steps.push("reload"); },
    async (saved) => {
      assert.equal(saved, account);
      steps.push("usage");
      return new Promise<void>(() => {});
    },
    () => assert.fail("unexpected usage error"),
  );
  assert.equal(result, account);
  assert.deepEqual(steps, ["saved", "reload", "usage"]);
});

test("import failure is reported and does not refresh usage", async () => {
  const failure = Error("import failed");
  let usageRefreshed = false;
  await assert.rejects(
    completeImportWithoutBlockingOnUsage(
      async () => { throw failure; },
      async () => undefined,
      async () => { usageRefreshed = true; },
      () => undefined,
    ),
    (error) => error === failure,
  );
  assert.equal(usageRefreshed, false);
});

test("background usage error does not make a saved import fail", async () => {
  const error = Error("usage offline");
  const errors: unknown[] = [];
  const account = await completeImportWithoutBlockingOnUsage(
    async () => ({ id: "saved" }),
    async () => undefined,
    async () => { throw error; },
    (e) => errors.push(e),
  );
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(account.id, "saved");
  assert.deepEqual(errors, [error]);
});
