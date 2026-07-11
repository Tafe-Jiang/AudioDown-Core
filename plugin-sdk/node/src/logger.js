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
            message: String(message),
            context,
          },
        })}\n`,
      );
    };
  }
  return logger;
}
