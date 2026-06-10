#!/usr/bin/env node
// Dev launcher — runs Tauri dev with isolated config dir + alternate proxy port,
// so a real LLM Relay GUI / agent can keep running alongside on the default port.
//
// Set LLM_RELAY_HOME to ~/.llm-relay-dev and LLM_RELAY_PROXY_PORT to 18081.
// Override either with env vars before invoking.

import { spawn } from "node:child_process";
import { homedir } from "node:os";
import { join } from "node:path";

const home = process.env.LLM_RELAY_HOME ?? join(homedir(), ".llm-relay-dev");
const port = process.env.LLM_RELAY_PROXY_PORT ?? "18081";

const env = {
  ...process.env,
  LLM_RELAY_HOME: home,
  LLM_RELAY_PROXY_PORT: port,
};

console.log(`[dev] LLM_RELAY_HOME=${home}`);
console.log(`[dev] LLM_RELAY_PROXY_PORT=${port}`);

const isWindows = process.platform === "win32";
const cmd = isWindows ? "pnpm.cmd" : "pnpm";
const child = spawn(cmd, ["tauri", "dev"], { env, stdio: "inherit", shell: false });
child.on("exit", (code) => process.exit(code ?? 0));
