import { rm } from "node:fs/promises";
import net from "node:net";

const sdkPath =
  process.env.AUDIODOWN_NODE_SDK_PATH ?? "/sdk/src/index.js";
const {
  CONTENT_METHODS,
  PluginContentError,
  createContentHandlers,
  createLogger,
  createPluginServer,
} = await import(sdkPath);
const manifest = (
  await import("../audiodown-plugin.json", { with: { type: "json" } })
).default;

const albumId = "catalog-album-1";
const canonicalId = "fixture:album:shared";
const handlers = createContentHandlers({
  [CONTENT_METHODS.SEARCH]: async ({ query, cursor }) => {
    if (query === "__hard_failure__") {
      throw new PluginContentError(
        "RESOURCE_ACCESS_DENIED",
        "Catalog resource access was denied",
      );
    }
    return {
      items: [
        albumItem(cursor === "search-page-2" ? "catalog-album-2" : albumId),
      ],
      ...(cursor === "search-page-2" ? {} : { nextCursor: "search-page-2" }),
    };
  },
  [CONTENT_METHODS.DISCOVER]: async () => ({
    sections: [
      {
        id: "catalog-ranked",
        title: "Catalog Ranked",
        layout: "ranked-list",
        items: [albumItem(albumId)],
      },
    ],
  }),
  [CONTENT_METHODS.CATEGORIES]: async () => ({
    items: [
      {
        resourceId: "catalog-category-1",
        canonicalId: "fixture:category:shared",
        title: "Catalog Category",
      },
    ],
  }),
  [CONTENT_METHODS.ALBUM_GET]: async ({ resourceId }) => ({
    album: {
      resourceId,
      canonicalId,
      title: "Catalog Album",
      creator: "Catalog Creator",
      trackCount: 2,
    },
  }),
  [CONTENT_METHODS.TRACKS_LIST]: async ({ cursor }) => {
    const secondPage = cursor === "tracks-page-2";
    return {
      items: [
        {
          resourceId: `catalog-track-${secondPage ? 2 : 1}`,
          title: `Catalog Track ${secondPage ? 2 : 1}`,
          sequence: secondPage ? 2 : 1,
          durationSeconds: 60,
        },
      ],
      ...(secondPage ? {} : { nextCursor: "tracks-page-2" }),
    };
  },
});

function albumItem(resourceId) {
  return {
    resourceType: "album",
    resourceId,
    canonicalId,
    title: "Catalog Album",
    subtitle: "Catalog Creator",
  };
}

const logger = createLogger({ output: process.stderr });
logger.info("virtual catalog content plugin ready", {
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
      logger.error("virtual catalog RPC failed", {
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
