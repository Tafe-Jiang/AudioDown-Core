const MAX_SCOPE_BYTES = 128;
const MAX_SCOPE_SEGMENTS = 8;
const MAX_SCOPE_SEGMENT_BYTES = 32;
const MAX_COOKIE_JAR_SESSION_ID_BYTES = 256;
const MAX_PLUGIN_OPAQUE_STATE_BYTES = 4 * 1024;
const MAX_QR_PAYLOAD_BYTES = 4 * 1024;
const MAX_QR_DISPLAY_CODE_BYTES = 128;
const MAX_ACCOUNT_TEXT_BYTES = 256;
const MAX_QR_EXPIRES_SECONDS = 60 * 60;
const MAX_POLL_INTERVAL_SECONDS = 60;
const MAX_ERROR_SUMMARY_BYTES = 512;
const MAX_RETRY_AFTER_SECONDS = 24 * 60 * 60;

export const CREDENTIAL_METHODS = Object.freeze({
  QR_START: "credential.qr.start",
  QR_POLL: "credential.qr.poll",
  IMPORT: "credential.import",
  STATUS: "credential.status",
  REFRESH: "credential.refresh",
  LOGOUT: "credential.logout",
});

const CREDENTIAL_METHOD_SET = new Set(Object.values(CREDENTIAL_METHODS));
const CREDENTIAL_STATUSES = new Set([
  "active",
  "expired",
  "revoked",
  "error",
]);
const QR_POLL_STATUSES = new Set([
  "pending",
  "scanned",
  "confirmed",
  "expired",
  "denied",
]);
const STANDARD_ERROR_CODES = new Set([
  "INVALID_REQUEST",
  "PLUGIN_NOT_FOUND",
  "PLUGIN_DISABLED",
  "PLUGIN_CAPABILITY_MISSING",
  "PLUGIN_UNAVAILABLE",
  "PLUGIN_TIMEOUT",
  "PLUGIN_RESPONSE_INVALID",
  "CREDENTIAL_NOT_FOUND",
  "CREDENTIAL_EXPIRED",
  "CREDENTIAL_SCOPE_NOT_ALLOWED",
  "LOGIN_FLOW_NOT_FOUND",
  "LOGIN_FLOW_EXPIRED",
  "LOGIN_PENDING",
  "LOGIN_DENIED",
  "RATE_LIMITED",
  "PLATFORM_RESPONSE_CHANGED",
  "PLUGIN_INTERNAL_ERROR",
]);

export class CredentialContractError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "CredentialContractError";
    this.code = code;
  }
}

export class PluginCredentialError extends Error {
  constructor(code, summary, retryAfterSeconds = undefined) {
    validatePluginError(code, summary, retryAfterSeconds);
    super(summary);
    this.name = "PluginCredentialError";
    this.code = code;
    this.summary = summary;
    this.retryAfterSeconds = retryAfterSeconds;
    Object.freeze(this);
  }
}

export function createCredentialHandlers(handlers) {
  assertPlainObject(handlers, "handlers", "INVALID_HANDLER");
  const wrapped = {};

  for (const [method, handler] of Object.entries(handlers)) {
    if (!CREDENTIAL_METHOD_SET.has(method) || typeof handler !== "function") {
      invalid("handler method", "INVALID_HANDLER");
    }
    wrapped[method] = async (params) => {
      validateRequest(method, params);
      const result = await handler(params);
      validateResult(method, result);
      return result;
    };
  }

  return Object.freeze(wrapped);
}

export function isCredentialMethod(method) {
  return CREDENTIAL_METHOD_SET.has(method);
}

function validateRequest(method, params) {
  assertPlainObject(params, "params", "INVALID_REQUEST");
  switch (method) {
    case CREDENTIAL_METHODS.QR_START:
      assertKeys(
        params,
        ["scope", "cookieJarSessionId"],
        "INVALID_REQUEST",
      );
      assertScope(params.scope, "scope", "INVALID_REQUEST");
      assertOpaque(
        params.cookieJarSessionId,
        MAX_COOKIE_JAR_SESSION_ID_BYTES,
        "cookieJarSessionId",
        "INVALID_REQUEST",
      );
      break;
    case CREDENTIAL_METHODS.QR_POLL:
      assertKeys(
        params,
        ["scope", "cookieJarSessionId", "pluginState"],
        "INVALID_REQUEST",
      );
      assertScope(params.scope, "scope", "INVALID_REQUEST");
      assertOpaque(
        params.cookieJarSessionId,
        MAX_COOKIE_JAR_SESSION_ID_BYTES,
        "cookieJarSessionId",
        "INVALID_REQUEST",
      );
      assertOptionalOpaque(
        params.pluginState,
        MAX_PLUGIN_OPAQUE_STATE_BYTES,
        "pluginState",
        "INVALID_REQUEST",
      );
      break;
    case CREDENTIAL_METHODS.IMPORT:
    case CREDENTIAL_METHODS.STATUS:
    case CREDENTIAL_METHODS.LOGOUT:
      assertKeys(params, ["credentialId", "scope"], "INVALID_REQUEST");
      assertCredentialId(params.credentialId, "INVALID_REQUEST");
      assertScope(params.scope, "scope", "INVALID_REQUEST");
      break;
    case CREDENTIAL_METHODS.REFRESH:
      assertKeys(
        params,
        ["credentialId", "scope", "cookieJarSessionId"],
        "INVALID_REQUEST",
      );
      assertCredentialId(params.credentialId, "INVALID_REQUEST");
      assertScope(params.scope, "scope", "INVALID_REQUEST");
      assertOpaque(
        params.cookieJarSessionId,
        MAX_COOKIE_JAR_SESSION_ID_BYTES,
        "cookieJarSessionId",
        "INVALID_REQUEST",
      );
      break;
    default:
      invalid("handler method", "INVALID_HANDLER");
  }
}

function validateResult(method, result) {
  assertPlainObject(result, "result", "PLUGIN_RESPONSE_INVALID");
  switch (method) {
    case CREDENTIAL_METHODS.QR_START:
      assertKeys(result, ["presentation"], "PLUGIN_RESPONSE_INVALID");
      assertQrPresentation(result.presentation);
      break;
    case CREDENTIAL_METHODS.QR_POLL:
      assertQrPollResult(result);
      break;
    case CREDENTIAL_METHODS.IMPORT:
    case CREDENTIAL_METHODS.STATUS:
      assertKeys(result, ["account"], "PLUGIN_RESPONSE_INVALID");
      assertAccount(result.account);
      break;
    case CREDENTIAL_METHODS.REFRESH:
      assertKeys(result, ["account"], "PLUGIN_RESPONSE_INVALID");
      assertAccount(result.account);
      if (result.account.status !== "active") {
        invalid("account status", "PLUGIN_RESPONSE_INVALID");
      }
      break;
    case CREDENTIAL_METHODS.LOGOUT:
      assertKeys(result, ["status"], "PLUGIN_RESPONSE_INVALID");
      if (result.status !== "revoked") {
        invalid("logout status", "PLUGIN_RESPONSE_INVALID");
      }
      break;
    default:
      invalid("handler method", "INVALID_HANDLER");
  }
}

function assertQrPresentation(presentation) {
  assertPlainObject(
    presentation,
    "presentation",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertKeys(
    presentation,
    [
      "payload",
      "displayCode",
      "expiresInSeconds",
      "pollIntervalSeconds",
      "pluginState",
    ],
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOpaque(
    presentation.payload,
    MAX_QR_PAYLOAD_BYTES,
    "qr payload",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalSafeText(
    presentation.displayCode,
    MAX_QR_DISPLAY_CODE_BYTES,
    "display code",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertIntegerRange(
    presentation.expiresInSeconds,
    1,
    MAX_QR_EXPIRES_SECONDS,
    "expiresInSeconds",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertPollInterval(
    presentation.pollIntervalSeconds,
    "PLUGIN_RESPONSE_INVALID",
  );
  if (presentation.pollIntervalSeconds > presentation.expiresInSeconds) {
    invalid("pollIntervalSeconds", "PLUGIN_RESPONSE_INVALID");
  }
  assertOptionalOpaque(
    presentation.pluginState,
    MAX_PLUGIN_OPAQUE_STATE_BYTES,
    "pluginState",
    "PLUGIN_RESPONSE_INVALID",
  );
}

function assertQrPollResult(result) {
  assertKeys(
    result,
    [
      "status",
      "nextPollSeconds",
      "pluginState",
      "promotion",
      "account",
    ],
    "PLUGIN_RESPONSE_INVALID",
  );
  if (!QR_POLL_STATUSES.has(result.status)) {
    invalid("poll status", "PLUGIN_RESPONSE_INVALID");
  }
  assertOptionalOpaque(
    result.pluginState,
    MAX_PLUGIN_OPAQUE_STATE_BYTES,
    "pluginState",
    "PLUGIN_RESPONSE_INVALID",
  );

  if (result.status === "pending" || result.status === "scanned") {
    if (isAbsent(result.nextPollSeconds)) {
      invalid("nextPollSeconds", "PLUGIN_RESPONSE_INVALID");
    }
    assertPollInterval(result.nextPollSeconds, "PLUGIN_RESPONSE_INVALID");
    if (!isAbsent(result.promotion) || !isAbsent(result.account)) {
      invalid("poll state", "PLUGIN_RESPONSE_INVALID");
    }
    return;
  }

  if (result.status === "confirmed") {
    if (
      !isAbsent(result.nextPollSeconds) ||
      isAbsent(result.promotion) ||
      isAbsent(result.account)
    ) {
      invalid("poll state", "PLUGIN_RESPONSE_INVALID");
    }
    assertPromotion(result.promotion);
    assertAccount(result.account);
    if (result.account.status !== "active") {
      invalid("account status", "PLUGIN_RESPONSE_INVALID");
    }
    return;
  }

  if (
    !isAbsent(result.nextPollSeconds) ||
    !isAbsent(result.promotion) ||
    !isAbsent(result.account)
  ) {
    invalid("poll state", "PLUGIN_RESPONSE_INVALID");
  }
}

function assertPromotion(promotion) {
  assertPlainObject(promotion, "promotion", "PLUGIN_RESPONSE_INVALID");
  assertKeys(promotion, ["scope"], "PLUGIN_RESPONSE_INVALID");
  assertScope(
    promotion.scope,
    "promotion scope",
    "PLUGIN_RESPONSE_INVALID",
  );
}

function assertAccount(account) {
  assertPlainObject(account, "account", "PLUGIN_RESPONSE_INVALID");
  assertKeys(
    account,
    ["status", "accountIdHint", "displayName"],
    "PLUGIN_RESPONSE_INVALID",
  );
  if (!CREDENTIAL_STATUSES.has(account.status)) {
    invalid("account status", "PLUGIN_RESPONSE_INVALID");
  }
  assertOptionalSafeText(
    account.accountIdHint,
    MAX_ACCOUNT_TEXT_BYTES,
    "accountIdHint",
    "PLUGIN_RESPONSE_INVALID",
  );
  assertOptionalSafeText(
    account.displayName,
    MAX_ACCOUNT_TEXT_BYTES,
    "displayName",
    "PLUGIN_RESPONSE_INVALID",
  );
}

function validatePluginError(code, summary, retryAfterSeconds) {
  if (!STANDARD_ERROR_CODES.has(code)) {
    invalid("error code", "INVALID_ERROR");
  }
  assertSafeText(
    summary,
    MAX_ERROR_SUMMARY_BYTES,
    "error summary",
    "INVALID_ERROR",
  );
  if (
    retryAfterSeconds !== undefined &&
    (!Number.isInteger(retryAfterSeconds) ||
      retryAfterSeconds < 0 ||
      retryAfterSeconds > MAX_RETRY_AFTER_SECONDS)
  ) {
    invalid("retryAfterSeconds", "INVALID_ERROR");
  }
}

function assertScope(value, field, code) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    Buffer.byteLength(value, "utf8") > MAX_SCOPE_BYTES ||
    !/^[\x00-\x7f]+$/.test(value)
  ) {
    invalid(field, code);
  }
  const segments = value.split(".");
  if (segments.length < 2 || segments.length > MAX_SCOPE_SEGMENTS) {
    invalid(field, code);
  }
  for (const segment of segments) {
    if (
      Buffer.byteLength(segment, "utf8") > MAX_SCOPE_SEGMENT_BYTES ||
      !/^[a-z][a-z0-9]*$/.test(segment)
    ) {
      invalid(field, code);
    }
  }
}

function assertCredentialId(value, code) {
  if (
    typeof value !== "string" ||
    !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(
      value,
    ) ||
    value.toLowerCase() === "00000000-0000-0000-0000-000000000000"
  ) {
    invalid("credentialId", code);
  }
}

function assertPlainObject(value, field, code) {
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.getPrototypeOf(value) !== Object.prototype
  ) {
    invalid(field, code);
  }
}

function assertKeys(value, allowed, code) {
  const allowedSet = new Set(allowed);
  if (Object.keys(value).some((key) => !allowedSet.has(key))) {
    invalid("unknown field", code);
  }
}

function assertOpaque(value, maximum, field, code) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.includes("\0") ||
    Buffer.byteLength(value, "utf8") > maximum
  ) {
    invalid(field, code);
  }
}

function assertOptionalOpaque(value, maximum, field, code) {
  if (!isAbsent(value)) {
    assertOpaque(value, maximum, field, code);
  }
}

function assertSafeText(value, maximum, field, code) {
  if (
    typeof value !== "string" ||
    value.trim().length === 0 ||
    Buffer.byteLength(value, "utf8") > maximum ||
    /\p{Cc}/u.test(value)
  ) {
    invalid(field, code);
  }
}

function assertOptionalSafeText(value, maximum, field, code) {
  if (!isAbsent(value)) {
    assertSafeText(value, maximum, field, code);
  }
}

function assertPollInterval(value, code) {
  assertIntegerRange(
    value,
    1,
    MAX_POLL_INTERVAL_SECONDS,
    "pollIntervalSeconds",
    code,
  );
}

function assertIntegerRange(value, minimum, maximum, field, code) {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    invalid(field, code);
  }
}

function isAbsent(value) {
  return value === undefined || value === null;
}

function invalid(field, code) {
  throw new CredentialContractError(code, `${field} is invalid`);
}
