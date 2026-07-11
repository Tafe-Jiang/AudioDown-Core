import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";
import test from "node:test";

import {
  CONTENT_METHODS,
  RpcError,
  createContentHandlers,
  createLogger,
  createPluginServer,
} from "../src/index.js";

const manifest = {
  id: "com.audiodown.virtual.content",
  version: "1.0.0",
};

function captureOutput() {
  let output = "";
  return {
    stream: new Writable({
      write(chunk, _encoding, callback) {
        output += chunk.toString();
        callback();
      },
    }),
    lines() {
      return output
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    },
  };
}

async function runServer(lines, handlers = {}) {
  const output = captureOutput();
  await createPluginServer({
    manifest,
    handlers,
    input: Readable.from(lines),
    output: output.stream,
  });
  return output.lines();
}

test("parses newline-delimited requests and writes one response per line", async () => {
  const handlers = createContentHandlers({
    [CONTENT_METHODS.SEARCH]: async ({ query }) => ({
      items: [
        {
          resourceType: "album",
          resourceId: query,
          title: `Result for ${query}`,
        },
      ],
    }),
  });
  const responses = await runServer([
    '{"jsonrpc":"2.0","id":"1","method":"content.search","params":{"query":"first","limit":20}}\n',
    '{"jsonrpc":"2.0","id":"2","method":"content.search","params":{"query":"second","limit":20}}\n',
  ], handlers);

  assert.equal(responses.length, 2);
  assert.equal(responses[0].result.items[0].resourceId, "first");
  assert.equal(responses[1].result.items[0].resourceId, "second");
});

test("rejects arbitrary registered handlers before serving requests", async () => {
  await assert.rejects(
    runServer([], { "system.shell": async () => ({}) }),
    /handler method is not allowed/,
  );
});

test("maps unexpected handler failures without leaking their messages", async () => {
  const handlers = createContentHandlers({
    [CONTENT_METHODS.SEARCH]: async () => {
      throw new Error("secret implementation detail");
    },
  });
  const [response] = await runServer([
    '{"jsonrpc":"2.0","id":"failure","method":"content.search","params":{"query":"virtual","limit":20}}\n',
  ], handlers);

  assert.equal(response.error.code, -32000);
  assert.equal(response.error.message, "Plugin call failed");
  assert.deepEqual(response.error.data, {
    code: "PLUGIN_INTERNAL_ERROR",
    summary: "Plugin call failed",
  });
  assert.equal(JSON.stringify(response).includes("secret implementation detail"), false);
});

test("provides protocol hello and health handlers", async () => {
  const responses = await runServer([
    '{"jsonrpc":"2.0","id":"hello","method":"system.hello","params":{}}\n',
    '{"jsonrpc":"2.0","id":"health","method":"system.health","params":{}}\n',
  ]);

  assert.equal(responses[0].result.pluginId, manifest.id);
  assert.equal(responses[0].result.pluginVersion, manifest.version);
  assert.equal(responses[0].result.protocolVersion, "1.0");
  assert.equal(responses[1].result.healthy, true);
  assert.equal(typeof responses[1].result.uptimeSeconds, "number");
});

test("rejects messages larger than one mebibyte", async () => {
  const output = captureOutput();
  await assert.rejects(
    createPluginServer({
      manifest,
      handlers: {},
      input: Readable.from(["x".repeat(1024 * 1024 + 1)]),
      output: output.stream,
    }),
    (error) => error instanceof RpcError && error.code === "MESSAGE_TOO_LARGE",
  );
});

test("emits structured logs as JSON-RPC notifications", () => {
  const output = captureOutput();
  const logger = createLogger({ output: output.stream });

  logger.info("virtual plugin ready", { healthy: true });

  const [notification] = output.lines();
  assert.equal(notification.jsonrpc, "2.0");
  assert.equal(notification.method, "log.emit");
  assert.equal(notification.params.level, "info");
  assert.equal(notification.params.message, "virtual plugin ready");
  assert.deepEqual(notification.params.context, { healthy: true });
});
