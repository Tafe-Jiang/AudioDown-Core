"use strict";

const { spawn } = process.getBuiltinModule("node:child_process");
const {
  cpSync,
  existsSync,
  rmSync,
  writeFileSync,
} = process.getBuiltinModule("node:fs");

const input = "/workspace/input";
const output = "/workspace/output";
const statusPath = "/workspace/status.json";
const allowLifecycleScripts =
  process.env.AUDIODOWN_ALLOW_LIFECYCLE_SCRIPTS === "true";
const npmArgs = ["ci", "--omit=dev"];
const maxLogBytes = 1024 * 1024;
const inputWaitAttempts = 300;

if (!allowLifecycleScripts) {
  npmArgs.push("--ignore-scripts");
}
npmArgs.push("--no-audit", "--no-fund");

function writeStatus(status) {
  writeFileSync(
    statusPath,
    `${JSON.stringify({ schemaVersion: "1.0", ...status })}\n`,
    { encoding: "utf8", mode: 0o600 },
  );
}

async function waitForInput() {
  for (let attempt = 0; attempt < inputWaitAttempts; attempt += 1) {
    if (existsSync(`${input}/.input-ready`)) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error("INPUT_NOT_READY");
}

async function run() {
  await waitForInput();
  rmSync(output, { recursive: true, force: true });
  cpSync(input, output, { recursive: true, dereference: false });
  rmSync(`${output}/.input-ready`, { force: true });

  const child = spawn("npm", npmArgs, {
    cwd: output,
    env: {
      HOME: "/workspace/home",
      HTTPS_PROXY: process.env.HTTPS_PROXY,
      HTTP_PROXY: process.env.HTTP_PROXY,
      NO_PROXY: process.env.NO_PROXY,
      NODE_ENV: "production",
      PATH: process.env.PATH,
      npm_config_cache: "/workspace/npm-cache",
    },
    shell: false,
    stdio: ["ignore", "pipe", "pipe"],
  });

  let logBytes = 0;
  let logLimitExceeded = false;
  const forward = (stream, destination) => {
    stream.on("data", (chunk) => {
      const remaining = Math.max(0, maxLogBytes - logBytes);
      if (remaining > 0) {
        destination.write(chunk.subarray(0, remaining));
      }
      logBytes += chunk.length;
      if (logBytes > maxLogBytes && !logLimitExceeded) {
        logLimitExceeded = true;
        child.kill("SIGKILL");
      }
    });
  };
  forward(child.stdout, process.stdout);
  forward(child.stderr, process.stderr);

  const status = await new Promise((resolve, reject) => {
    child.once("error", reject);
    child.once("close", resolve);
  });
  if (logLimitExceeded) {
    throw new Error("BUILD_LOG_LIMIT_EXCEEDED");
  }
  if (status !== 0) {
    throw new Error(`NPM_CI_EXIT_${status ?? "UNKNOWN"}`);
  }
}

run().then(() => {
  writeStatus({ state: "completed" });
}).catch((error) => {
  const message = error instanceof Error ? error.message : "unknown build failure";
  writeStatus({
    state: "failed",
    code: message === "BUILD_LOG_LIMIT_EXCEEDED"
      ? "BUILD_LOG_LIMIT_EXCEEDED"
      : "BUILD_RUNNER_FAILED",
    message,
  });
});

setInterval(() => {}, 60_000);
