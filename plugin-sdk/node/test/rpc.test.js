import assert from "node:assert/strict";
import { Readable, Writable } from "node:stream";
import test from "node:test";

import {
  CONTENT_METHODS,
  CREDENTIAL_METHODS,
  PluginCredentialError,
  RpcError,
  createContentHandlers,
  createCredentialHandlers,
  createPluginServer,
} from "../src/index.js";

const manifest = {
  id: "com.audiodown.virtual.credential",
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

test("registers only phase-three content and phase-four credential handlers", async () => {
  const handlers = {
    ...createContentHandlers({
      [CONTENT_METHODS.CATEGORIES]: async () => ({ items: [] }),
    }),
    ...createCredentialHandlers({
      [CREDENTIAL_METHODS.STATUS]: async () => ({
        account: { status: "active" },
      }),
    }),
  };

  const responses = await runServer(
    [
      '{"jsonrpc":"2.0","id":"content","method":"content.categories","params":{}}\n',
      '{"jsonrpc":"2.0","id":"credential","method":"credential.status","params":{"credentialId":"8d86182f-95f7-44d8-a75c-b9d1ec2c18ad","scope":"virtual.web"}}\n',
    ],
    handlers,
  );
  assert.deepEqual(responses[0].result, { items: [] });
  assert.equal(responses[1].result.account.status, "active");

  await assert.rejects(
    runServer([], { "credential.export": async () => ({}) }),
    /handler method is not allowed/,
  );
});

test("maps safe credential plugin errors to application errors", async () => {
  const handlers = createCredentialHandlers({
    [CREDENTIAL_METHODS.QR_POLL]: async () => {
      throw new PluginCredentialError(
        "LOGIN_PENDING",
        "Virtual login is pending",
        2,
      );
    },
  });
  const [response] = await runServer(
    [
      '{"jsonrpc":"2.0","id":"pending","method":"credential.qr.poll","params":{"scope":"virtual.web","cookieJarSessionId":"jar-session-1"}}\n',
    ],
    handlers,
  );

  assert.equal(response.error.code, -32000);
  assert.equal(response.error.message, "Virtual login is pending");
  assert.deepEqual(response.error.data, {
    code: "LOGIN_PENDING",
    summary: "Virtual login is pending",
    retryAfterSeconds: 2,
  });
});

test("redacts unexpected credential failures and invalid responses", async () => {
  const unexpected = createCredentialHandlers({
    [CREDENTIAL_METHODS.STATUS]: async () => {
      throw new Error("credential implementation secret");
    },
  });
  const [unexpectedResponse] = await runServer(
    [
      '{"jsonrpc":"2.0","id":"failure","method":"credential.status","params":{"credentialId":"8d86182f-95f7-44d8-a75c-b9d1ec2c18ad","scope":"virtual.web"}}\n',
    ],
    unexpected,
  );
  assert.deepEqual(unexpectedResponse.error.data, {
    code: "PLUGIN_INTERNAL_ERROR",
    summary: "Plugin call failed",
  });
  assert.equal(
    JSON.stringify(unexpectedResponse).includes("credential implementation secret"),
    false,
  );

  const invalid = createCredentialHandlers({
    [CREDENTIAL_METHODS.LOGOUT]: async () => ({
      status: "revoked",
      token: "must-not-leak",
    }),
  });
  const [invalidResponse] = await runServer(
    [
      '{"jsonrpc":"2.0","id":"invalid","method":"credential.logout","params":{"credentialId":"8d86182f-95f7-44d8-a75c-b9d1ec2c18ad","scope":"virtual.web"}}\n',
    ],
    invalid,
  );
  assert.deepEqual(invalidResponse.error.data, {
    code: "PLUGIN_RESPONSE_INVALID",
    summary: "Plugin response was invalid",
  });
  assert.equal(JSON.stringify(invalidResponse).includes("must-not-leak"), false);
});

test("does not execute inherited handlers outside the allowlist", async () => {
  const inheritedHandlers = Object.create({
    "credential.export": async () => ({ exported: true }),
  });

  await assert.rejects(
    runServer(
      [
        '{"jsonrpc":"2.0","id":"export","method":"credential.export","params":{}}\n',
      ],
      inheritedHandlers,
    ),
    /handler method is not allowed/,
  );
});

test("does not execute non-enumerable handlers outside the allowlist", async () => {
  const hiddenHandlers = {};
  Object.defineProperty(hiddenHandlers, "credential.export", {
    enumerable: false,
    value: async () => ({ exported: true }),
  });

  await assert.rejects(
    runServer(
      [
        '{"jsonrpc":"2.0","id":"export","method":"credential.export","params":{}}\n',
      ],
      hiddenHandlers,
    ),
    /handler method is not allowed/,
  );
});

test("redacts arbitrary RpcError data and mutated plugin errors", async () => {
  const proxyToken = "rpc-proxy-token-canary-0123456789";
  const previousToken = process.env.AUDIODOWN_PROXY_TOKEN;
  process.env.AUDIODOWN_PROXY_TOKEN = proxyToken;
  try {
    const arbitrary = createCredentialHandlers({
      [CREDENTIAL_METHODS.STATUS]: async () => {
        throw new RpcError(-32099, proxyToken, { proxyToken });
      },
    });
    const [arbitraryResponse] = await runServer(
      [
        '{"jsonrpc":"2.0","id":"arbitrary","method":"credential.status","params":{"credentialId":"8d86182f-95f7-44d8-a75c-b9d1ec2c18ad","scope":"virtual.web"}}\n',
      ],
      arbitrary,
    );
    assert.deepEqual(arbitraryResponse.error.data, {
      code: "PLUGIN_INTERNAL_ERROR",
      summary: "Plugin call failed",
    });
    assert.equal(JSON.stringify(arbitraryResponse).includes(proxyToken), false);

    const mutated = createCredentialHandlers({
      [CREDENTIAL_METHODS.STATUS]: async () => {
        const error = new PluginCredentialError(
          "LOGIN_DENIED",
          "Virtual login was denied",
        );
        error.summary = proxyToken;
        error.message = proxyToken;
        throw error;
      },
    });
    const [mutatedResponse] = await runServer(
      [
        '{"jsonrpc":"2.0","id":"mutated","method":"credential.status","params":{"credentialId":"8d86182f-95f7-44d8-a75c-b9d1ec2c18ad","scope":"virtual.web"}}\n',
      ],
      mutated,
    );
    assert.equal(JSON.stringify(mutatedResponse).includes(proxyToken), false);

    const encoded = createCredentialHandlers({
      [CREDENTIAL_METHODS.STATUS]: async () => {
        throw new PluginCredentialError(
          "LOGIN_DENIED",
          Buffer.from(proxyToken).toString("base64"),
        );
      },
    });
    const [encodedResponse] = await runServer(
      [
        '{"jsonrpc":"2.0","id":"encoded","method":"credential.status","params":{"credentialId":"8d86182f-95f7-44d8-a75c-b9d1ec2c18ad","scope":"virtual.web"}}\n',
      ],
      encoded,
    );
    assert.equal(
      JSON.stringify(encodedResponse).includes(
        Buffer.from(proxyToken).toString("base64"),
      ),
      false,
    );

    const reflectedResult = createCredentialHandlers({
      [CREDENTIAL_METHODS.QR_START]: async () => ({
        presentation: {
          payload: proxyToken,
          expiresInSeconds: 300,
          pollIntervalSeconds: 2,
        },
      }),
    });
    const [reflectedResponse] = await runServer(
      [
        '{"jsonrpc":"2.0","id":"reflected","method":"credential.qr.start","params":{"scope":"virtual.web","cookieJarSessionId":"jar-session-1"}}\n',
      ],
      reflectedResult,
    );
    assert.deepEqual(reflectedResponse.error.data, {
      code: "PLUGIN_RESPONSE_INVALID",
      summary: "Plugin response was invalid",
    });
    assert.equal(JSON.stringify(reflectedResponse).includes(proxyToken), false);
  } finally {
    if (previousToken === undefined) {
      delete process.env.AUDIODOWN_PROXY_TOKEN;
    } else {
      process.env.AUDIODOWN_PROXY_TOKEN = previousToken;
    }
  }
});
