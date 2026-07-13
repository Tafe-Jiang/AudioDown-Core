import {
  ContentContractError,
  PluginContentError,
  isContentMethod,
} from "./content.js";
import {
  CredentialContractError,
  PluginCredentialError,
  isCredentialMethod,
} from "./credential.js";

const PROTOCOL_VERSION = "1.0";
const MAX_MESSAGE_BYTES = 1024 * 1024;
const APPLICATION_ERROR = -32000;

export class RpcError extends Error {
  constructor(code, message, data = undefined) {
    super(message);
    this.name = "RpcError";
    this.code = code;
    this.data = data;
  }
}

export async function createPluginServer({
  manifest,
  handlers,
  input,
  output,
}) {
  validateHandlerMap(handlers);
  const startedAt = Date.now();
  let buffer = "";
  let shuttingDown = false;

  const builtInHandlers = {
    "system.hello": async () => ({
      pluginId: manifest.id,
      pluginVersion: manifest.version,
      protocolVersion: PROTOCOL_VERSION,
    }),
    "system.health": async () => ({
      pluginId: manifest.id,
      pluginVersion: manifest.version,
      protocolVersion: PROTOCOL_VERSION,
      healthy: true,
      uptimeSeconds: Math.max(0, Math.floor((Date.now() - startedAt) / 1000)),
    }),
    "system.shutdown": async () => {
      shuttingDown = true;
      return { stopping: true };
    },
  };

  async function processLine(line) {
    let request;
    try {
      request = JSON.parse(line);
    } catch {
      await writeJson(output, errorResponse(null, -32700, "Parse error"));
      return;
    }

    const id = request?.id ?? null;
    if (
      request?.jsonrpc !== "2.0" ||
      typeof request?.method !== "string" ||
      !Object.hasOwn(request, "id")
    ) {
      await writeJson(output, errorResponse(id, -32600, "Invalid request"));
      return;
    }

    const handler = Object.hasOwn(builtInHandlers, request.method)
      ? builtInHandlers[request.method]
      : Object.hasOwn(handlers, request.method)
        ? handlers[request.method]
        : undefined;
    if (typeof handler !== "function") {
      await writeJson(output, errorResponse(id, -32601, "Method not found"));
      return;
    }

    try {
      const result = await handler(request.params ?? {});
      if (containsProxyToken(result)) {
        throw new CredentialContractError(
          "PLUGIN_RESPONSE_INVALID",
          "plugin result contains the proxy token",
        );
      }
      await writeJson(output, {
        jsonrpc: "2.0",
        id,
        result: result ?? null,
      });
    } catch (error) {
      const rpcError = safeRpcError(error);
      await writeJson(
        output,
        errorResponse(id, rpcError.code, rpcError.message, rpcError.data),
      );
    }
  }

  for await (const chunk of input) {
    buffer += chunk.toString("utf8");

    let newlineIndex;
    while ((newlineIndex = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, newlineIndex).replace(/\r$/, "");
      buffer = buffer.slice(newlineIndex + 1);
      if (line.length > 0) {
        if (Buffer.byteLength(line, "utf8") > MAX_MESSAGE_BYTES) {
          throw new RpcError("MESSAGE_TOO_LARGE", "RPC message exceeds 1 MiB");
        }
        await processLine(line);
      }
      if (shuttingDown) {
        return;
      }
      if (Buffer.byteLength(buffer, "utf8") > MAX_MESSAGE_BYTES) {
        throw new RpcError("MESSAGE_TOO_LARGE", "RPC message exceeds 1 MiB");
      }
    }
  }

  if (buffer.length > 0) {
    if (Buffer.byteLength(buffer, "utf8") > MAX_MESSAGE_BYTES) {
      throw new RpcError("MESSAGE_TOO_LARGE", "RPC message exceeds 1 MiB");
    }
    await processLine(buffer);
  }
}

function validateHandlerMap(handlers) {
  if (
    handlers === null ||
    typeof handlers !== "object" ||
    Array.isArray(handlers) ||
    ![Object.prototype, null].includes(Object.getPrototypeOf(handlers)) ||
    Reflect.ownKeys(handlers).some((method) => {
      if (
        typeof method !== "string" ||
        (!isContentMethod(method) && !isCredentialMethod(method))
      ) {
        return true;
      }
      const descriptor = Object.getOwnPropertyDescriptor(handlers, method);
      return descriptor === undefined || typeof descriptor.value !== "function";
    })
  ) {
    throw new Error("handler method is not allowed");
  }
}

function safeRpcError(error) {
  if (error instanceof PluginContentError) {
    if (containsProxyToken(error.summary)) {
      return internalError();
    }
    return new RpcError(APPLICATION_ERROR, error.summary, {
      code: error.code,
      summary: error.summary,
      ...(error.retryAfterSeconds === undefined
        ? {}
        : { retryAfterSeconds: error.retryAfterSeconds }),
    });
  }
  if (error instanceof PluginCredentialError) {
    if (containsProxyToken(error.summary)) {
      return internalError();
    }
    return new RpcError(APPLICATION_ERROR, error.summary, {
      code: error.code,
      summary: error.summary,
      ...(error.retryAfterSeconds === undefined
        ? {}
        : { retryAfterSeconds: error.retryAfterSeconds }),
    });
  }
  if (
    error instanceof ContentContractError ||
    error instanceof CredentialContractError
  ) {
    const requestError = error.code === "INVALID_REQUEST";
    const code = requestError ? "INVALID_REQUEST" : "PLUGIN_RESPONSE_INVALID";
    const summary = requestError
      ? "Plugin request was invalid"
      : "Plugin response was invalid";
    return new RpcError(APPLICATION_ERROR, summary, { code, summary });
  }
  return internalError();
}

function containsProxyToken(value) {
  const token = process.env.AUDIODOWN_PROXY_TOKEN;
  if (typeof token !== "string" || token.length === 0) {
    return false;
  }
  const candidates = [
    token,
    Buffer.from(token, "utf8").toString("base64"),
  ];
  if (typeof value === "string") {
    return candidates.some((candidate) => value.includes(candidate));
  }
  if (Array.isArray(value)) {
    return value.some(containsProxyToken);
  }
  if (value !== null && typeof value === "object") {
    return Object.entries(value).some(
      ([key, item]) => containsProxyToken(key) || containsProxyToken(item),
    );
  }
  return false;
}

function internalError() {
  return new RpcError(APPLICATION_ERROR, "Plugin call failed", {
    code: "PLUGIN_INTERNAL_ERROR",
    summary: "Plugin call failed",
  });
}

function errorResponse(id, code, message, data = undefined) {
  return {
    jsonrpc: "2.0",
    id,
    error: {
      code,
      message,
      ...(data === undefined ? {} : { data }),
    },
  };
}

async function writeJson(output, value) {
  const line = `${JSON.stringify(value)}\n`;
  if (!output.write(line)) {
    await new Promise((resolve) => output.once("drain", resolve));
  }
}
