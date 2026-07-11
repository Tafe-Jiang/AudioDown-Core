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

await createPluginServer({
  manifest,
  handlers: {},
  input: process.stdin,
  output: process.stdout,
});
