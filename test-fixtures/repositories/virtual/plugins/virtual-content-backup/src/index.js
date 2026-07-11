import { rm } from "node:fs/promises";
import net from "node:net";

const sdkPath =
  process.env.AUDIODOWN_NODE_SDK_PATH ?? "/sdk/src/index.js";
const {
  CONTENT_METHODS,
  createContentHandlers,
  createLogger,
  createPluginServer,
} = await import(sdkPath);
const manifest = (
  await import("../audiodown-plugin.json", { with: { type: "json" } })
).default;

const albumId = "virtual-backup-album-1";
const canonicalId = "fixture:album:shared";
const handlers = createContentHandlers({
  [CONTENT_METHODS.SEARCH]: async ({ query, cursor }) => ({
    items: [
      albumItem(
        cursor === "search-page-2"
          ? "virtual-backup-album-2"
          : albumId,
        query,
      ),
    ],
    ...(cursor === "search-page-2" ? {} : { nextCursor: "search-page-2" }),
  }),
  [CONTENT_METHODS.DISCOVER]: async () => ({
    sections: [
      {
        id: "backup-albums",
        title: "Backup Albums",
        layout: "album-grid",
        items: [albumItem(albumId, "discover")],
      },
    ],
  }),
  [CONTENT_METHODS.CATEGORIES]: async () => ({
    items: [
      {
        resourceId: "virtual-backup-category-1",
        canonicalId: "fixture:category:shared",
        title: "Virtual Backup Category",
      },
    ],
  }),
  [CONTENT_METHODS.ALBUM_GET]: async ({ resourceId }) => ({
    album: {
      resourceId,
      canonicalId,
      title: "Virtual Backup Album",
      creator: "Virtual Backup Creator",
      trackCount: 2,
    },
  }),
  [CONTENT_METHODS.TRACKS_LIST]: async ({ cursor }) => {
    const secondPage = cursor === "tracks-page-2";
    return {
      items: [
        {
          resourceId: `virtual-backup-track-${secondPage ? 2 : 1}`,
          title: `Virtual Backup Track ${secondPage ? 2 : 1}`,
          sequence: secondPage ? 2 : 1,
          durationSeconds: 60,
        },
      ],
      ...(secondPage ? {} : { nextCursor: "tracks-page-2" }),
    };
  },
});

function albumItem(resourceId, query) {
  return {
    resourceType: "album",
    resourceId,
    canonicalId,
    title: "Virtual Backup Album",
    subtitle: `Virtual backup result for ${query}`,
  };
}

const logger = createLogger({ output: process.stderr });
logger.info("virtual backup content plugin ready", {
  pluginId: manifest.id,
  version: manifest.version,
});

const socketPath = process.env.AUDIODOWN_RPC_SOCKET;
if (socketPath) {
  await rm(socketPath, { force: true });
  const server = net.createServer((socket) => {
    createPluginServer({
      manifest,
      handlers,
      input: socket,
      output: socket,
    }).catch((error) => {
      logger.error("virtual backup RPC failed", {
        error: error instanceof Error ? error.message : String(error),
      });
      socket.destroy();
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(socketPath, resolve);
  });
} else {
  await createPluginServer({
    manifest,
    handlers,
    input: process.stdin,
    output: process.stdout,
  });
}
