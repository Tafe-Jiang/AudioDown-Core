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

const profile = {
  albumId: "virtual-album-1",
  secondAlbumId: "virtual-album-2",
  title: "Virtual Primary Album",
  creator: "Virtual Primary Creator",
  canonicalId: "fixture:album:shared",
  categoryId: "virtual-category-1",
  trackPrefix: "virtual-primary-track",
};

const handlers = createContentHandlers({
  [CONTENT_METHODS.SEARCH]: async ({ query, cursor }) => {
    if (query === "__retryable__") {
      throw new PluginContentError(
        "RATE_LIMITED",
        "Virtual source asked Core to retry",
        1,
      );
    }
    if (query === "__hard_failure__") {
      throw new PluginContentError(
        "RESOURCE_ACCESS_DENIED",
        "Virtual resource access was denied",
      );
    }
    if (query === "__delay__") {
      await new Promise((resolve) => setTimeout(resolve, 150));
    }
    const secondPage = cursor === "search-page-2";
    return {
      items: [
        albumItem(secondPage ? profile.secondAlbumId : profile.albumId),
      ],
      ...(secondPage ? {} : { nextCursor: "search-page-2" }),
    };
  },
  [CONTENT_METHODS.DISCOVER]: async () => ({
    sections: [
      discoverSection("hero", "Hero", "hero-carousel", profile.albumId),
      discoverSection("albums", "Albums", "album-grid", profile.albumId),
      discoverSection(
        "recent",
        "Recent",
        "horizontal-list",
        profile.secondAlbumId,
      ),
      discoverSection("ranked", "Ranked", "ranked-list", profile.albumId),
      {
        id: "categories",
        title: "Categories",
        layout: "category-grid",
        items: [
          {
            resourceType: "category",
            resourceId: profile.categoryId,
            canonicalId: "fixture:category:shared",
            title: "Virtual Category",
          },
        ],
      },
    ],
  }),
  [CONTENT_METHODS.CATEGORIES]: async () => ({
    items: [
      {
        resourceId: profile.categoryId,
        canonicalId: "fixture:category:shared",
        title: "Virtual Category",
        description: "Deterministic local category",
      },
    ],
  }),
  [CONTENT_METHODS.ALBUM_GET]: async ({ resourceId }) => ({
    album: {
      resourceId,
      canonicalId: profile.canonicalId,
      title: profile.title,
      creator: profile.creator,
      description: "Deterministic local album",
      trackCount: 2,
    },
  }),
  [CONTENT_METHODS.TRACKS_LIST]: async ({ cursor }) => {
    const secondPage = cursor === "tracks-page-2";
    return {
      items: [
        {
          resourceId: `${profile.trackPrefix}-${secondPage ? 2 : 1}`,
          canonicalId: `fixture:track:${secondPage ? 2 : 1}`,
          title: `Virtual Track ${secondPage ? 2 : 1}`,
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
    canonicalId: profile.canonicalId,
    title: profile.title,
    subtitle: profile.creator,
    description: "Deterministic local result",
  };
}

function discoverSection(id, title, layout, resourceId) {
  return {
    id,
    title,
    layout,
    items: [albumItem(resourceId)],
  };
}

const logger = createLogger({ output: process.stderr });
logger.info("virtual content plugin ready", {
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
      logger.error("virtual content RPC failed", {
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
