import assert from "node:assert/strict";
import test from "node:test";

import {
  CREDENTIAL_METHODS,
  CredentialContractError,
  PluginCredentialError,
  createCredentialHandlers,
} from "../src/index.js";

const credentialId = "8d86182f-95f7-44d8-a75c-b9d1ec2c18ad";
const scope = "virtual.web";

function activeAccount() {
  return {
    status: "active",
    accountIdHint: "virtual-account-1",
    displayName: "Virtual Account",
  };
}

test("exports only the six phase-four credential methods", () => {
  assert.deepEqual(Object.values(CREDENTIAL_METHODS), [
    "credential.qr.start",
    "credential.qr.poll",
    "credential.import",
    "credential.status",
    "credential.refresh",
    "credential.logout",
  ]);
  assert.equal(Object.isFrozen(CREDENTIAL_METHODS), true);
});

test("wraps typed credential handlers and validates all six contracts", async () => {
  const calls = [];
  const handlers = createCredentialHandlers({
    [CREDENTIAL_METHODS.QR_START]: async (params) => {
      calls.push([CREDENTIAL_METHODS.QR_START, params]);
      return {
        presentation: {
          payload: "virtual-qr-payload",
          displayCode: "ABCD-1234",
          expiresInSeconds: 300,
          pollIntervalSeconds: 2,
          pluginState: "state-1",
        },
      };
    },
    [CREDENTIAL_METHODS.QR_POLL]: async (params) => {
      calls.push([CREDENTIAL_METHODS.QR_POLL, params]);
      return {
        status: "confirmed",
        promotion: { scope },
        account: activeAccount(),
      };
    },
    [CREDENTIAL_METHODS.IMPORT]: async (params) => {
      calls.push([CREDENTIAL_METHODS.IMPORT, params]);
      return { account: activeAccount() };
    },
    [CREDENTIAL_METHODS.STATUS]: async (params) => {
      calls.push([CREDENTIAL_METHODS.STATUS, params]);
      return {
        account: {
          status: "expired",
          accountIdHint: "virtual-account-1",
        },
      };
    },
    [CREDENTIAL_METHODS.REFRESH]: async (params) => {
      calls.push([CREDENTIAL_METHODS.REFRESH, params]);
      return { account: activeAccount() };
    },
    [CREDENTIAL_METHODS.LOGOUT]: async (params) => {
      calls.push([CREDENTIAL_METHODS.LOGOUT, params]);
      return { status: "revoked" };
    },
  });

  await handlers[CREDENTIAL_METHODS.QR_START]({
    scope,
    cookieJarSessionId: "jar-session-1",
  });
  await handlers[CREDENTIAL_METHODS.QR_POLL]({
    scope,
    cookieJarSessionId: "jar-session-1",
    pluginState: "state-1",
  });
  await handlers[CREDENTIAL_METHODS.IMPORT]({ credentialId, scope });
  await handlers[CREDENTIAL_METHODS.STATUS]({ credentialId, scope });
  await handlers[CREDENTIAL_METHODS.REFRESH]({
    credentialId,
    scope,
    cookieJarSessionId: "refresh-jar-1",
  });
  await handlers[CREDENTIAL_METHODS.LOGOUT]({ credentialId, scope });

  assert.equal(calls.length, 6);
  assert.equal(Object.isFrozen(handlers), true);
});

test("rejects unknown handlers, fields, malformed identities, and plaintext", async () => {
  assert.throws(
    () => createCredentialHandlers({ "credential.export": async () => ({}) }),
    (error) =>
      error instanceof CredentialContractError &&
      error.code === "INVALID_HANDLER",
  );

  const handlers = createCredentialHandlers({
    [CREDENTIAL_METHODS.IMPORT]: async () => ({ account: activeAccount() }),
  });

  for (const params of [
    { credentialId, scope, cookie: "plaintext-cookie" },
    {
      credentialId,
      scope,
      headers: { Authorization: "Bearer plaintext-token" },
    },
    { credentialId: "not-a-uuid", scope },
    { credentialId, scope: "Virtual.web" },
  ]) {
    await assert.rejects(
      handlers[CREDENTIAL_METHODS.IMPORT](params),
      (error) =>
        error instanceof CredentialContractError &&
        error.code === "INVALID_REQUEST",
    );
  }
});

test("enforces declarative QR limits and poll state consistency", async () => {
  const oversizedPayload = createCredentialHandlers({
    [CREDENTIAL_METHODS.QR_START]: async () => ({
      presentation: {
        payload: "x".repeat(4097),
        expiresInSeconds: 300,
        pollIntervalSeconds: 2,
      },
    }),
  });
  await assert.rejects(
    oversizedPayload[CREDENTIAL_METHODS.QR_START]({
      scope,
      cookieJarSessionId: "jar-session-1",
    }),
    (error) =>
      error instanceof CredentialContractError &&
      error.code === "PLUGIN_RESPONSE_INVALID",
  );

  const oversizedState = createCredentialHandlers({
    [CREDENTIAL_METHODS.QR_POLL]: async () => ({
      status: "pending",
      nextPollSeconds: 2,
      pluginState: "x".repeat(4097),
    }),
  });
  await assert.rejects(
    oversizedState[CREDENTIAL_METHODS.QR_POLL]({
      scope,
      cookieJarSessionId: "jar-session-1",
    }),
    (error) =>
      error instanceof CredentialContractError &&
      error.code === "PLUGIN_RESPONSE_INVALID",
  );

  for (const result of [
    { status: "pending" },
    {
      status: "pending",
      nextPollSeconds: 2,
      promotion: { scope },
    },
    {
      status: "confirmed",
      promotion: { scope },
      account: { status: "expired" },
    },
    { status: "expired", nextPollSeconds: 2 },
  ]) {
    const handlers = createCredentialHandlers({
      [CREDENTIAL_METHODS.QR_POLL]: async () => result,
    });
    await assert.rejects(
      handlers[CREDENTIAL_METHODS.QR_POLL]({
        scope,
        cookieJarSessionId: "jar-session-1",
      }),
      (error) =>
        error instanceof CredentialContractError &&
        error.code === "PLUGIN_RESPONSE_INVALID",
    );
  }
});

test("requires active refresh and revoked logout results", async () => {
  const handlers = createCredentialHandlers({
    [CREDENTIAL_METHODS.REFRESH]: async () => ({
      account: { status: "error" },
    }),
    [CREDENTIAL_METHODS.LOGOUT]: async () => ({ status: "active" }),
  });

  await assert.rejects(
    handlers[CREDENTIAL_METHODS.REFRESH]({
      credentialId,
      scope,
      cookieJarSessionId: "refresh-jar-1",
    }),
    (error) =>
      error instanceof CredentialContractError &&
      error.code === "PLUGIN_RESPONSE_INVALID",
  );
  await assert.rejects(
    handlers[CREDENTIAL_METHODS.LOGOUT]({ credentialId, scope }),
    (error) =>
      error instanceof CredentialContractError &&
      error.code === "PLUGIN_RESPONSE_INVALID",
  );
});

test("creates only bounded standard credential plugin errors", () => {
  const error = new PluginCredentialError(
    "LOGIN_PENDING",
    "The virtual login is still pending",
    2,
  );
  assert.equal(error.code, "LOGIN_PENDING");
  assert.equal(error.retryAfterSeconds, 2);

  assert.throws(
    () => new PluginCredentialError("NOT_A_CODE", "unsafe"),
    (candidate) =>
      candidate instanceof CredentialContractError &&
      candidate.code === "INVALID_ERROR",
  );
  assert.throws(
    () => new PluginCredentialError("PLUGIN_INTERNAL_ERROR", "x".repeat(513)),
    (candidate) =>
      candidate instanceof CredentialContractError &&
      candidate.code === "INVALID_ERROR",
  );
});
