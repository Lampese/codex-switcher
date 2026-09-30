import assert from "node:assert/strict";
import test from "node:test";
import type { Update } from "@tauri-apps/plugin-updater";
import {
  installUpdate,
  type UpdateInstallationStatus,
} from "../src/lib/updateInstallation.ts";

function deferred() {
  let resolve!: () => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

test("Finished waits for installation before offering a restart", async () => {
  const installation = deferred();
  const states: UpdateInstallationStatus[] = [];
  const update: Pick<Update, "downloadAndInstall"> = {
    async downloadAndInstall(onEvent) {
      onEvent?.({ event: "Started", data: { contentLength: 100 } });
      onEvent?.({ event: "Progress", data: { chunkLength: 100 } });
      onEvent?.({ event: "Finished" });
      await installation.promise;
    },
  };

  const pending = installUpdate(update, (state) => states.push(state));
  await Promise.resolve();
  assert.deepEqual(states.at(-1), { kind: "installing" });
  assert.equal(states.some((state) => state.kind === "ready"), false);

  installation.resolve();
  await pending;
  assert.deepEqual(states.at(-1), { kind: "ready" });
  assert.equal(states.filter((state) => state.kind === "ready").length, 1);
});

test("an installation failure after Finished never offers a restart", async () => {
  const installation = deferred();
  const states: UpdateInstallationStatus[] = [];
  const error = new Error("Permission denied (os error 13)");
  const update: Pick<Update, "downloadAndInstall"> = {
    async downloadAndInstall(onEvent) {
      onEvent?.({ event: "Finished" });
      await installation.promise;
    },
  };

  const pending = installUpdate(update, (state) => states.push(state));
  const rejected = assert.rejects(pending, (err) => err === error);
  installation.reject(error);
  await rejected;
  assert.deepEqual(states.at(-1), { kind: "installing" });
  assert.equal(states.some((state) => state.kind === "ready"), false);
});

test("a download failure is propagated without reporting installation success", async () => {
  const states: UpdateInstallationStatus[] = [];
  const error = new Error("download failed");
  await assert.rejects(
    installUpdate(
      { async downloadAndInstall() { throw error; } },
      (state) => states.push(state),
    ),
    (err) => err === error,
  );
  assert.deepEqual(states, [{ kind: "downloading", downloaded: 0, total: null }]);
});

test("progress accumulates bytes when the content length is unknown", async () => {
  const states: UpdateInstallationStatus[] = [];
  await installUpdate(
    {
      async downloadAndInstall(onEvent) {
        onEvent?.({ event: "Started", data: {} });
        onEvent?.({ event: "Progress", data: { chunkLength: 30 } });
        onEvent?.({ event: "Progress", data: { chunkLength: 70 } });
        onEvent?.({ event: "Finished" });
      },
    },
    (state) => states.push(state),
  );
  assert.deepEqual(states, [
    { kind: "downloading", downloaded: 0, total: null },
    { kind: "downloading", downloaded: 0, total: null },
    { kind: "downloading", downloaded: 30, total: null },
    { kind: "downloading", downloaded: 100, total: null },
    { kind: "installing" },
    { kind: "ready" },
  ]);
});

test("a zero content length is not treated as missing", async () => {
  const states: UpdateInstallationStatus[] = [];
  await installUpdate(
    {
      async downloadAndInstall(onEvent) {
        onEvent?.({ event: "Started", data: { contentLength: 0 } });
        onEvent?.({ event: "Finished" });
      },
    },
    (state) => states.push(state),
  );
  assert.deepEqual(states[1], { kind: "downloading", downloaded: 0, total: 0 });
});

test("installation completion is authoritative even without progress events", async () => {
  const installation = deferred();
  const states: UpdateInstallationStatus[] = [];
  const pending = installUpdate(
    { downloadAndInstall: () => installation.promise },
    (state) => states.push(state),
  );
  assert.deepEqual(states, [{ kind: "downloading", downloaded: 0, total: null }]);
  installation.resolve();
  await pending;
  assert.deepEqual(states.at(-1), { kind: "ready" });
});
