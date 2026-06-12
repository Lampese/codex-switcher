import { existsSync } from "node:fs";
import { join, delimiter } from "node:path";
import { homedir } from "node:os";
import { spawn, spawnSync } from "node:child_process";

function canRun(command) {
  const result = spawnSync(command, ["--version"], {
    stdio: "ignore",
    env: process.env,
  });

  return result.status === 0;
}

function prependPath(path) {
  process.env.PATH = `${path}${delimiter}${process.env.PATH ?? ""}`;
  process.env.Path = process.env.PATH;
}

if (!canRun("cargo")) {
  const cargoBin = join(homedir(), ".cargo", "bin");
  if (existsSync(cargoBin)) {
    prependPath(cargoBin);
  }
}

if (!canRun("cargo")) {
  console.error("Error: cargo not found. Install Rust via rustup: https://rustup.rs");
  process.exit(1);
}

const tauriCli = join(process.cwd(), "node_modules", "@tauri-apps", "cli", "tauri.js");

if (!existsSync(tauriCli)) {
  console.error("Error: Tauri CLI not found. Run pnpm install.");
  process.exit(1);
}

const child = spawn(process.execPath, [tauriCli, ...process.argv.slice(2)], {
  stdio: "inherit",
  env: process.env,
});

child.on("exit", (code, signal) => {
  if (signal) {
    process.kill(process.pid, signal);
    return;
  }

  process.exit(code ?? 1);
});

child.on("error", (error) => {
  console.error(error.message);
  process.exit(1);
});
