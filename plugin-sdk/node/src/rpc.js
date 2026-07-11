const PROTOCOL_VERSION = "1.0";
const MAX_MESSAGE_BYTES = 1024 * 1024;

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

    const handler = builtInHandlers[request.method] ?? handlers[request.method];
    if (typeof handler !== "function") {
      await writeJson(output, errorResponse(id, -32601, "Method not found"));
      return;
    }

    try {
      const result = await handler(request.params ?? {});
      await writeJson(output, {
        jsonrpc: "2.0",
        id,
        result: result ?? null,
      });
    } catch (error) {
      const rpcError =
        error instanceof RpcError
          ? error
          : new RpcError(-32603, "Internal error");
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
