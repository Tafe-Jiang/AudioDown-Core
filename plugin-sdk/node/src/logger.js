const LEVELS = ["trace", "debug", "info", "warn", "error"];

export function createLogger({ output }) {
  const logger = {};
  for (const level of LEVELS) {
    logger[level] = (message, context = {}) => {
      output.write(
        `${JSON.stringify({
          jsonrpc: "2.0",
          method: "log.emit",
          params: {
            level,
            message: redactProxyToken(String(message)),
            context: redactProxyToken(context),
          },
        })}\n`,
      );
    };
  }
  return logger;
}

function redactProxyToken(value, seen = new WeakSet()) {
  const token = process.env.AUDIODOWN_PROXY_TOKEN;
  if (typeof token !== "string" || token.length === 0) {
    return value;
  }
  const candidates = [
    token,
    Buffer.from(token, "utf8").toString("base64"),
  ];

  if (typeof value === "string") {
    return candidates.reduce(
      (redacted, candidate) =>
        redacted.split(candidate).join("[REDACTED]"),
      value,
    );
  }
  if (value === null || typeof value !== "object") {
    return value;
  }
  if (seen.has(value)) {
    return "[REDACTED]";
  }
  seen.add(value);
  if (Array.isArray(value)) {
    return value.map((item) => redactProxyToken(item, seen));
  }

  const redacted = {};
  for (const [key, item] of Object.entries(value)) {
    redacted[redactProxyToken(key, seen)] = redactProxyToken(item, seen);
  }
  return redacted;
}
