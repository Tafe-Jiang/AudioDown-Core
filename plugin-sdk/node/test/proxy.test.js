import assert from "node:assert/strict";
import test from "node:test";

import {
  ProxyContractError,
  ProxyError,
  createProxyClient,
} from "../src/index.js";

const proxyUrl = "http://audiodown-gateway:18081";
const proxyToken = "proxy-token-canary-0123456789abcdef";

async function withProxyEnvironment(callback) {
  const previousUrl = process.env.AUDIODOWN_PROXY_URL;
  const previousToken = process.env.AUDIODOWN_PROXY_TOKEN;
  process.env.AUDIODOWN_PROXY_URL = proxyUrl;
  process.env.AUDIODOWN_PROXY_TOKEN = proxyToken;
  try {
    return await callback();
  } finally {
    if (previousUrl === undefined) {
      delete process.env.AUDIODOWN_PROXY_URL;
    } else {
      process.env.AUDIODOWN_PROXY_URL = previousUrl;
    }
    if (previousToken === undefined) {
      delete process.env.AUDIODOWN_PROXY_TOKEN;
    } else {
      process.env.AUDIODOWN_PROXY_TOKEN = previousToken;
    }
  }
}

function validRequest(overrides = {}) {
  return {
    requestId: "request-1",
    method: "GET",
    url: "https://service.virtual.invalid/account",
    headers: { accept: "application/json" },
    bodyBase64: null,
    cookieJarSessionId: null,
    credentialScope: null,
    ...overrides,
  };
}

function gatewayResponse(overrides = {}) {
  return {
    status: 200,
    headers: { "content-type": "application/json" },
    bodyBase64: "e30=",
    error: null,
    ...overrides,
  };
}

test("frames one bounded JSON request to the fixed environment gateway", async () => {
  await withProxyEnvironment(async () => {
    const calls = [];
    const client = createProxyClient({
      fetchImpl: async (url, init) => {
        calls.push([url, init]);
        return new Response(JSON.stringify(gatewayResponse()), {
          status: 200,
          headers: { "content-type": "application/json" },
        });
      },
    });

    const result = await client.request(validRequest());
    assert.deepEqual(result, gatewayResponse());
    assert.equal(calls.length, 1);
    assert.equal(calls[0][0], `${proxyUrl}/`);
    assert.equal(calls[0][1].method, "POST");
    assert.equal(calls[0][1].headers["content-type"], "application/json");
    assert.deepEqual(JSON.parse(calls[0][1].body), {
      token: proxyToken,
      requestId: "request-1",
      method: "GET",
      url: "https://service.virtual.invalid/account",
      headers: { accept: "application/json" },
      bodyBase64: null,
      cookieJarSessionId: null,
      credentialScope: null,
    });

    assert.equal(JSON.stringify(client).includes(proxyToken), false);
    assert.equal(JSON.stringify(result).includes(proxyToken), false);
  });
});

test("accepts no caller-selected gateway URL or token", async () => {
  await withProxyEnvironment(async () => {
    for (const options of [
      { gatewayUrl: "http://alternate.invalid" },
      { token: "caller-token" },
    ]) {
      assert.throws(
        () => createProxyClient(options),
        (error) =>
          error instanceof ProxyContractError &&
          error.code === "PROXY_CONFIGURATION_INVALID",
      );
    }
  });
});

test("requires one exact internal HTTP origin from the environment", async () => {
  await withProxyEnvironment(async () => {
    for (const invalid of [
      "https://audiodown-gateway:18081",
      "http://user:pass@audiodown-gateway:18081",
      "http://audiodown-gateway:18081/path",
      "http://audiodown-gateway:18081/?query=1",
      "http://audiodown-gateway:18081/#fragment",
    ]) {
      process.env.AUDIODOWN_PROXY_URL = invalid;
      assert.throws(
        () => createProxyClient(),
        (error) =>
          error instanceof ProxyContractError &&
          error.code === "PROXY_CONFIGURATION_INVALID",
      );
    }
  });
});

test("rejects sensitive headers in requests and gateway results", async () => {
  await withProxyEnvironment(async () => {
    let fetchCalls = 0;
    const client = createProxyClient({
      fetchImpl: async () => {
        fetchCalls += 1;
        return new Response(
          JSON.stringify(
            gatewayResponse({
              headers: { "Set-Cookie": "session=must-not-be-visible" },
            }),
          ),
          { status: 200 },
        );
      },
    });

    for (const name of ["Cookie", "Authorization", "Set-Cookie"]) {
      await assert.rejects(
        client.request(validRequest({ headers: { [name]: "secret" } })),
        (error) =>
          error instanceof ProxyContractError &&
          error.code === "INVALID_REQUEST",
      );
    }
    assert.equal(fetchCalls, 0);

    await assert.rejects(
      client.request(validRequest()),
      (error) =>
        error instanceof ProxyContractError &&
        error.code === "PROXY_RESPONSE_INVALID",
    );
  });
});

test("bounds request and response messages before exposing them", async () => {
  await withProxyEnvironment(async () => {
    let fetchCalls = 0;
    const requestClient = createProxyClient({
      fetchImpl: async () => {
        fetchCalls += 1;
        return new Response(JSON.stringify(gatewayResponse()), { status: 200 });
      },
    });
    await assert.rejects(
      requestClient.request(
        validRequest({
          bodyBase64: Buffer.alloc(800_000, "x").toString("base64"),
        }),
      ),
      (error) =>
        error instanceof ProxyContractError &&
        error.code === "MESSAGE_TOO_LARGE",
    );
    assert.equal(fetchCalls, 0);

    const responseClient = createProxyClient({
      fetchImpl: async () =>
        new Response("x".repeat(1024 * 1024 + 1), { status: 200 }),
    });
    await assert.rejects(
      responseClient.request(validRequest()),
      (error) =>
        error instanceof ProxyContractError &&
        error.code === "MESSAGE_TOO_LARGE",
    );
  });
});

test("times out safely and redacts the proxy token from all errors", async () => {
  await withProxyEnvironment(async () => {
    const timeoutClient = createProxyClient({
      timeoutMs: 10,
      fetchImpl: async (_url, { signal }) =>
        new Promise((_resolve, reject) => {
          signal.addEventListener(
            "abort",
            () => reject(new Error(`aborted ${proxyToken}`)),
            { once: true },
          );
        }),
    });

    await assert.rejects(
      timeoutClient.request(validRequest()),
      (error) =>
        error instanceof ProxyError &&
        error.code === "PROXY_TIMEOUT" &&
        !String(error).includes(proxyToken) &&
        !JSON.stringify(error).includes(proxyToken),
    );

    const transportClient = createProxyClient({
      fetchImpl: async () => {
        throw new Error(`transport leaked ${proxyToken}`);
      },
    });
    await assert.rejects(
      transportClient.request(validRequest()),
      (error) =>
        error instanceof ProxyError &&
        error.code === "PROXY_UNAVAILABLE" &&
        !String(error).includes(proxyToken) &&
        !JSON.stringify(error).includes(proxyToken),
    );
  });
});

test("rejects any gateway response that reflects the proxy token", async () => {
  await withProxyEnvironment(async () => {
    const client = createProxyClient({
      fetchImpl: async () =>
        new Response(
          JSON.stringify(
            gatewayResponse({
              bodyBase64: Buffer.from(proxyToken).toString("base64"),
            }),
          ),
          { status: 200 },
        ),
    });

    await assert.rejects(
      client.request(validRequest()),
      (error) =>
        error instanceof ProxyContractError &&
        error.code === "PROXY_RESPONSE_INVALID" &&
        !String(error).includes(proxyToken),
    );
  });
});

test("rejects proxy token reflection hidden behind JSON Unicode escapes", async () => {
  await withProxyEnvironment(async () => {
    for (const reflected of [
      proxyToken,
      Buffer.from(proxyToken).toString("base64"),
    ]) {
      const escaped = [...reflected]
        .map((character) =>
          `\\u${character.codePointAt(0).toString(16).padStart(4, "0")}`)
        .join("");
      const client = createProxyClient({
        fetchImpl: async () =>
          new Response(
            `{"status":502,"headers":{},"bodyBase64":null,"error":{"code":"PLUGIN_INTERNAL_ERROR","summary":"${escaped}","retryAfterSeconds":null}}`,
            { status: 200 },
          ),
      });

      await assert.rejects(
        client.request(validRequest()),
        (error) =>
          error instanceof ProxyContractError &&
          error.code === "PROXY_RESPONSE_INVALID" &&
          !String(error).includes(proxyToken),
      );
    }
  });
});

test("accepts safe gateway errors without a retry interval", async () => {
  await withProxyEnvironment(async () => {
    const client = createProxyClient({
      fetchImpl: async () =>
        new Response(
          JSON.stringify(
            gatewayResponse({
              status: 403,
              bodyBase64: null,
              error: {
                code: "CREDENTIAL_SCOPE_NOT_ALLOWED",
                summary: "Credential scope is not allowed",
              },
            }),
          ),
          { status: 200 },
        ),
    });

    const result = await client.request(validRequest());
    assert.deepEqual(result.error, {
      code: "CREDENTIAL_SCOPE_NOT_ALLOWED",
      summary: "Credential scope is not allowed",
    });
  });
});
