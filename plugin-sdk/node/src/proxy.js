const MAX_MESSAGE_BYTES = 1024 * 1024;
const MAX_TOKEN_BYTES = 4 * 1024;
const MAX_REQUEST_ID_BYTES = 256;
const MAX_URL_BYTES = 8 * 1024;
const MAX_HEADER_COUNT = 64;
const MAX_HEADER_NAME_BYTES = 128;
const MAX_HEADER_VALUE_BYTES = 8 * 1024;
const MAX_ERROR_CODE_BYTES = 64;
const MAX_ERROR_SUMMARY_BYTES = 512;
const MAX_RETRY_AFTER_SECONDS = 24 * 60 * 60;
const DEFAULT_TIMEOUT_MS = 10_000;
const MAX_TIMEOUT_MS = 60_000;

const ALLOWED_METHODS = new Set([
  "GET",
  "HEAD",
  "POST",
  "PUT",
  "PATCH",
  "DELETE",
]);
const SENSITIVE_HEADERS = new Set([
  "authorization",
  "cookie",
  "set-cookie",
]);

export class ProxyError extends Error {
  constructor(code, message) {
    super(message);
    this.name = "ProxyError";
    this.code = code;
  }
}

export class ProxyContractError extends ProxyError {
  constructor(code, message) {
    super(code, message);
    this.name = "ProxyContractError";
  }
}

export function createProxyClient(options = {}) {
  assertOptions(options);
  const gatewayUrl = readGatewayUrl();
  const token = readProxyToken();
  const fetchImpl = options.fetchImpl ?? globalThis.fetch;
  const timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;

  if (typeof fetchImpl !== "function") {
    configurationInvalid();
  }
  if (
    !Number.isInteger(timeoutMs) ||
    timeoutMs < 1 ||
    timeoutMs > MAX_TIMEOUT_MS
  ) {
    configurationInvalid();
  }

  return Object.freeze({
    async request(request) {
      const normalized = normalizeRequest(request);
      const frame = {
        token,
        requestId: normalized.requestId,
        method: normalized.method,
        url: normalized.url,
        headers: normalized.headers,
        bodyBase64: normalized.bodyBase64,
        cookieJarSessionId: normalized.cookieJarSessionId,
        credentialScope: normalized.credentialScope,
      };
      const body = JSON.stringify(frame);
      assertMessageSize(body);

      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(), timeoutMs);

      try {
        const response = await fetchImpl(gatewayUrl, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body,
          signal: controller.signal,
          redirect: "error",
        });
        const raw = await readBoundedResponse(response);
        if (
          raw.includes(token) ||
          raw.includes(Buffer.from(token, "utf8").toString("base64"))
        ) {
          responseInvalid();
        }

        let value;
        try {
          value = JSON.parse(raw);
        } catch {
          responseInvalid();
        }
        const normalized = normalizeResponse(value);
        if (containsToken(normalized, token)) {
          responseInvalid();
        }
        return normalized;
      } catch (error) {
        if (error instanceof ProxyContractError) {
          throw error;
        }
        if (controller.signal.aborted) {
          throw new ProxyError("PROXY_TIMEOUT", "Core proxy request timed out");
        }
        throw new ProxyError("PROXY_UNAVAILABLE", "Core proxy is unavailable");
      } finally {
        clearTimeout(timer);
      }
    },
  });
}

function assertOptions(options) {
  assertPlainObject(
    options,
    "PROXY_CONFIGURATION_INVALID",
    configurationInvalid,
  );
  const allowed = new Set(["fetchImpl", "timeoutMs"]);
  if (Object.keys(options).some((key) => !allowed.has(key))) {
    configurationInvalid();
  }
}

function readGatewayUrl() {
  const raw = process.env.AUDIODOWN_PROXY_URL;
  if (typeof raw !== "string" || raw.length === 0) {
    configurationInvalid();
  }

  let parsed;
  try {
    parsed = new URL(raw);
  } catch {
    configurationInvalid();
  }
  if (
    parsed.protocol !== "http:" ||
    parsed.username !== "" ||
    parsed.password !== "" ||
    parsed.pathname !== "/" ||
    parsed.search !== "" ||
    parsed.hash !== "" ||
    (raw !== parsed.origin && raw !== `${parsed.origin}/`)
  ) {
    configurationInvalid();
  }
  return `${parsed.origin}/`;
}

function readProxyToken() {
  const token = process.env.AUDIODOWN_PROXY_TOKEN;
  if (
    typeof token !== "string" ||
    token.length === 0 ||
    token.includes("\0") ||
    Buffer.byteLength(token, "utf8") > MAX_TOKEN_BYTES
  ) {
    configurationInvalid();
  }
  return token;
}

function normalizeRequest(request) {
  assertPlainObject(request, "INVALID_REQUEST", requestInvalid);
  assertExactKeys(request, [
    "requestId",
    "method",
    "url",
    "headers",
    "bodyBase64",
    "cookieJarSessionId",
    "credentialScope",
  ], requestInvalid);

  assertOpaque(
    request.requestId,
    MAX_REQUEST_ID_BYTES,
    requestInvalid,
  );
  if (!ALLOWED_METHODS.has(request.method)) {
    requestInvalid();
  }
  assertTargetUrl(request.url);
  const headers = normalizeHeaders(request.headers, requestInvalid);
  assertNullableBase64(request.bodyBase64, requestInvalid);
  assertNullableOpaque(
    request.cookieJarSessionId,
    256,
    requestInvalid,
  );
  assertNullableScope(request.credentialScope, requestInvalid);

  return {
    requestId: request.requestId,
    method: request.method,
    url: request.url,
    headers,
    bodyBase64: request.bodyBase64,
    cookieJarSessionId: request.cookieJarSessionId,
    credentialScope: request.credentialScope,
  };
}

function normalizeResponse(response) {
  assertPlainObject(response, "PROXY_RESPONSE_INVALID", responseInvalid);
  assertExactKeys(
    response,
    ["status", "headers", "bodyBase64", "error"],
    responseInvalid,
  );
  if (
    !Number.isInteger(response.status) ||
    response.status < 100 ||
    response.status > 599
  ) {
    responseInvalid();
  }
  const headers = normalizeHeaders(response.headers, responseInvalid);
  assertNullableBase64(response.bodyBase64, responseInvalid);
  const error = normalizeGatewayError(response.error);
  return Object.freeze({
    status: response.status,
    headers: Object.freeze(headers),
    bodyBase64: response.bodyBase64,
    error,
  });
}

function normalizeGatewayError(error) {
  if (error === null) {
    return null;
  }
  assertPlainObject(error, "PROXY_RESPONSE_INVALID", responseInvalid);
  assertAllowedKeys(
    error,
    ["code", "summary", "retryAfterSeconds"],
    responseInvalid,
  );
  if (!Object.hasOwn(error, "code") || !Object.hasOwn(error, "summary")) {
    responseInvalid();
  }
  if (
    typeof error.code !== "string" ||
    Buffer.byteLength(error.code, "utf8") > MAX_ERROR_CODE_BYTES ||
    !/^[A-Z][A-Z0-9_]*$/.test(error.code)
  ) {
    responseInvalid();
  }
  assertSafeText(
    error.summary,
    MAX_ERROR_SUMMARY_BYTES,
    responseInvalid,
  );
  if (
    error.retryAfterSeconds !== undefined &&
    error.retryAfterSeconds !== null &&
    (!Number.isInteger(error.retryAfterSeconds) ||
      error.retryAfterSeconds < 0 ||
      error.retryAfterSeconds > MAX_RETRY_AFTER_SECONDS)
  ) {
    responseInvalid();
  }
  return Object.freeze({
    code: error.code,
    summary: error.summary,
    ...(error.retryAfterSeconds === undefined ||
    error.retryAfterSeconds === null
      ? {}
      : { retryAfterSeconds: error.retryAfterSeconds }),
  });
}

async function readBoundedResponse(response) {
  if (
    response === null ||
    typeof response !== "object" ||
    typeof response.status !== "number"
  ) {
    responseInvalid();
  }

  const contentLength = response.headers?.get?.("content-length");
  if (
    contentLength !== null &&
    contentLength !== undefined &&
    (/^\d+$/.test(contentLength) === false ||
      Number(contentLength) > MAX_MESSAGE_BYTES)
  ) {
    messageTooLarge();
  }

  if (response.body?.getReader) {
    const reader = response.body.getReader();
    const chunks = [];
    let total = 0;
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) {
          break;
        }
        const chunk = Buffer.from(value);
        total += chunk.length;
        if (total > MAX_MESSAGE_BYTES) {
          await reader.cancel();
          messageTooLarge();
        }
        chunks.push(chunk);
      }
    } finally {
      reader.releaseLock();
    }
    return Buffer.concat(chunks, total).toString("utf8");
  }

  if (typeof response.arrayBuffer !== "function") {
    responseInvalid();
  }
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.length > MAX_MESSAGE_BYTES) {
    messageTooLarge();
  }
  return bytes.toString("utf8");
}

function normalizeHeaders(headers, onInvalid) {
  assertPlainObject(headers, "INVALID_HEADERS", onInvalid);
  const entries = Object.entries(headers);
  if (entries.length > MAX_HEADER_COUNT) {
    onInvalid();
  }

  const normalized = {};
  for (const [name, value] of entries) {
    const lowerName = name.toLowerCase();
    if (
      name.length === 0 ||
      Buffer.byteLength(name, "utf8") > MAX_HEADER_NAME_BYTES ||
      !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(name) ||
      SENSITIVE_HEADERS.has(lowerName) ||
      Object.hasOwn(normalized, lowerName) ||
      typeof value !== "string" ||
      Buffer.byteLength(value, "utf8") > MAX_HEADER_VALUE_BYTES ||
      /[\r\n\0]/.test(value)
    ) {
      onInvalid();
    }
    normalized[lowerName] = value;
  }
  return normalized;
}

function assertTargetUrl(value) {
  if (
    typeof value !== "string" ||
    Buffer.byteLength(value, "utf8") > MAX_URL_BYTES
  ) {
    requestInvalid();
  }
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    requestInvalid();
  }
  if (
    !["http:", "https:"].includes(parsed.protocol) ||
    parsed.username !== "" ||
    parsed.password !== "" ||
    parsed.hash !== ""
  ) {
    requestInvalid();
  }
}

function assertNullableScope(value, onInvalid) {
  if (value === null) {
    return;
  }
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    Buffer.byteLength(value, "utf8") > 128
  ) {
    onInvalid();
  }
  const segments = value.split(".");
  if (segments.length < 2 || segments.length > 8) {
    onInvalid();
  }
  if (
    segments.some(
      (segment) =>
        Buffer.byteLength(segment, "utf8") > 32 ||
        !/^[a-z][a-z0-9]*$/.test(segment),
    )
  ) {
    onInvalid();
  }
}

function assertNullableOpaque(value, maximum, onInvalid) {
  if (value !== null) {
    assertOpaque(value, maximum, onInvalid);
  }
}

function assertOpaque(value, maximum, onInvalid) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.includes("\0") ||
    Buffer.byteLength(value, "utf8") > maximum
  ) {
    onInvalid();
  }
}

function assertNullableBase64(value, onInvalid) {
  if (value === null) {
    return;
  }
  if (typeof value !== "string" || value.length % 4 !== 0) {
    onInvalid();
  }
  const decoded = Buffer.from(value, "base64");
  if (decoded.toString("base64") !== value) {
    onInvalid();
  }
}

function assertSafeText(value, maximum, onInvalid) {
  if (
    typeof value !== "string" ||
    value.trim().length === 0 ||
    Buffer.byteLength(value, "utf8") > maximum ||
    /\p{Cc}/u.test(value)
  ) {
    onInvalid();
  }
}

function assertPlainObject(value, _code, onInvalid) {
  if (
    value === null ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.getPrototypeOf(value) !== Object.prototype
  ) {
    onInvalid();
  }
}

function assertExactKeys(value, expected, onInvalid) {
  const actual = Object.keys(value);
  if (
    actual.length !== expected.length ||
    actual.some((key) => !expected.includes(key))
  ) {
    onInvalid();
  }
}

function assertAllowedKeys(value, allowed, onInvalid) {
  if (Object.keys(value).some((key) => !allowed.includes(key))) {
    onInvalid();
  }
}

function containsToken(value, token) {
  if (typeof value === "string") {
    return (
      value.includes(token) ||
      value.includes(Buffer.from(token, "utf8").toString("base64"))
    );
  }
  if (Array.isArray(value)) {
    return value.some((item) => containsToken(item, token));
  }
  if (value !== null && typeof value === "object") {
    return Object.entries(value).some(
      ([key, item]) => key.includes(token) || containsToken(item, token),
    );
  }
  return false;
}

function assertMessageSize(value) {
  if (Buffer.byteLength(value, "utf8") > MAX_MESSAGE_BYTES) {
    messageTooLarge();
  }
}

function configurationInvalid() {
  throw new ProxyContractError(
    "PROXY_CONFIGURATION_INVALID",
    "Core proxy configuration is invalid",
  );
}

function requestInvalid() {
  throw new ProxyContractError(
    "INVALID_REQUEST",
    "Core proxy request is invalid",
  );
}

function responseInvalid() {
  throw new ProxyContractError(
    "PROXY_RESPONSE_INVALID",
    "Core proxy response is invalid",
  );
}

function messageTooLarge() {
  throw new ProxyContractError(
    "MESSAGE_TOO_LARGE",
    "Core proxy message exceeds 1 MiB",
  );
}
