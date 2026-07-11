import { rm } from "node:fs/promises";
import net from "node:net";

const sdkPath =
  process.env.AUDIODOWN_NODE_SDK_PATH ?? "/sdk/src/index.js";
const { createLogger, createPluginServer } = await import(sdkPath);
const manifest = (
  await import("../audiodown-plugin.json", { with: { type: "json" } })
).default;

const logger = createLogger({ output: process.stderr });
logger.info("virtual plugin ready", {
  pluginId: manifest.id,
  version: manifest.version,
});

const socketPath = process.env.AUDIODOWN_RPC_SOCKET;
if (socketPath) {
  await rm(socketPath, { force: true });
  const server = net.createServer((socket) => {
    createPluginServer({
      manifest,
      handlers: {},
      input: socket,
      output: socket,
    }).catch((error) => {
      logger.error("virtual plugin RPC failed", {
        error: error instanceof Error ? error.message : String(error),
      });
      socket.destroy();
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
} else {
  await createPluginServer({
    manifest,
    handlers: {},
    input: process.stdin,
    output: process.stdout,
  });
}
