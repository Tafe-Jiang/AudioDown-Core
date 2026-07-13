import type { Page, Route } from "@playwright/test";

export const longPluginId =
  "com.audiodown.virtual.content.with-an-intentionally-long-identifier-for-responsive-verification";
export const longCommitSha = "0123456789abcdef0123456789abcdef01234567";
export const longLogMessage =
  "虚拟插件在握手阶段返回了一段很长的结构化错误摘要，用于验证移动端日志内容能够自然换行且不会覆盖时间、级别或后续内容。";
export const discoverPluginId = "com.audiodown.virtual.content";
export const discoverAlbumResourceId = "virtual-album-hero";
export const discoverAlbumTitle = "Virtual Primary Album";
export const discoverTrackCursor = "opaque-tracks-page-2";

const repositoryUrl =
  "https://github.com/example-owner/example-audiodown-plugin-repository";
const discoverPluginName = "Virtual Content";
const discoverPluginVersion = "1.0.0";
const discoverSource = {
  platformId: "virtual",
  pluginId: discoverPluginId,
  pluginName: discoverPluginName,
  pluginVersion: discoverPluginVersion,
};
const catalogSource = {
  platformId: "catalog",
  pluginId: "com.audiodown.catalog.content",
  pluginName: "Virtual Catalog",
  pluginVersion: "1.0.0",
};

export interface MockApiOptions {
  supervisorAvailable?: boolean;
  developmentMode?: boolean;
  repositoryInspectionError?: boolean;
  repositoryRisk?: boolean;
  plugins?: "empty" | "installed";
  logs?: "empty" | "populated";
  search?: "empty" | "results" | "partial";
  discover?: "empty" | "results" | "partial";
}

function fulfillJson(route: Route, value: unknown, status = 200) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(value),
  });
}

function discoverItem(
  resourceType: "album" | "track" | "category",
  resourceId: string,
  title: string,
  subtitle?: string,
) {
  return {
    resourceType,
    resourceId,
    canonicalId: `fixture:${resourceType}:${resourceId}`,
    title,
    subtitle,
    description:
      "Deterministic virtual content with verylongunbrokenmetadatavalueforresponsivechecks",
  };
}

function discoverSections() {
  return [
    {
      section: {
        id: "hero",
        title: "Featured",
        layout: "hero-carousel",
        items: [
          discoverItem(
            "album",
            discoverAlbumResourceId,
            "Virtual Hero Album",
            "Virtual Primary Creator",
          ),
        ],
      },
      source: discoverSource,
    },
    {
      section: {
        id: "albums",
        title: "Albums",
        layout: "album-grid",
        items: [
          discoverItem(
            "album",
            "virtual-album-grid",
            "Virtual Grid Album",
            "Virtual Grid Creator",
          ),
        ],
      },
      source: discoverSource,
    },
    {
      section: {
        id: "recent",
        title: "Recent",
        layout: "horizontal-list",
        items: [
          discoverItem(
            "album",
            "virtual-album-recent",
            "Virtual Recent Album",
            "Virtual Recent Creator",
          ),
        ],
      },
      source: discoverSource,
    },
    {
      section: {
        id: "ranked",
        title: "Ranked",
        layout: "ranked-list",
        items: [
          discoverItem(
            "track",
            "virtual-track-ranked",
            "Virtual Ranked Track",
            "Virtual Ranked Creator",
          ),
        ],
      },
      source: discoverSource,
    },
    {
      section: {
        id: "categories",
        title: "Categories",
        layout: "category-grid",
        items: [
          discoverItem(
            "category",
            "virtual-category-discover",
            "Virtual Discover Category",
          ),
        ],
      },
      source: discoverSource,
    },
  ];
}

export async function mockCoreApi(
  page: Page,
  options: MockApiOptions = {},
) {
  const supervisorAvailable = options.supervisorAvailable ?? true;
  const useDiscoverPlugin =
    options.discover === "results" || options.discover === "partial";
  let plugins =
    options.plugins === "empty"
      ? []
      : [
          {
            pluginId: useDiscoverPlugin ? discoverPluginId : longPluginId,
            pluginType: "content",
            platformId: useDiscoverPlugin ? "virtual" : "virtual-content",
            name: useDiscoverPlugin
              ? discoverPluginName
              : "Virtual Content Plugin With A Long Responsive Name",
            version: "1.0.0",
            status: "installed",
            enabled: true,
            runMode: "on_demand",
            priority: 100,
            sourceUrl: repositoryUrl,
            commitSha: longCommitSha,
            capabilities: [
              "content.search",
              "content.discover",
              "content.categories",
              "content.album.get",
              "content.tracks.list",
            ],
            searchEnabled: true,
            discoverEnabled: true,
            isDefaultContentPlugin: true,
          },
        ];

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const { pathname } = url;
    const method = request.method();

    if (pathname === "/api/v1/system") {
      return fulfillJson(route, {
        version: "1.0.0-alpha.1",
        supervisor: {
          available: supervisorAvailable,
          error: supervisorAvailable ? null : "Supervisor is unavailable",
        },
        pluginCount: plugins.length,
        developmentMode: options.developmentMode ?? true,
      });
    }

    if (pathname === "/api/v1/search") {
      if (method !== "GET" || !url.searchParams.get("q")) {
        return fulfillJson(
          route,
          {
            code: "INVALID_SEARCH_REQUEST",
            message: "Search request is invalid",
          },
          400,
        );
      }
      if (options.search === "results" || options.search === "partial") {
        const cursor = url.searchParams.get("cursor");
        if (cursor && cursor !== "opaque-search-page-2") {
          return fulfillJson(
            route,
            { code: "INVALID_CURSOR", message: "Cursor is invalid" },
            400,
          );
        }
        const secondPage = cursor === "opaque-search-page-2";
        return fulfillJson(route, {
          items: secondPage
            ? [
                {
                  item: {
                    resourceType: "album",
                    resourceId: "virtual-album-2",
                    canonicalId: "fixture:album:page-2",
                    title: "Virtual Search Album Page Two",
                    subtitle: "Virtual Creator",
                  },
                  source: {
                    platformId: "virtual-content",
                    pluginId: longPluginId,
                    pluginName:
                      "Virtual Content Plugin With A Long Responsive Name",
                    pluginVersion: "1.0.0",
                  },
                },
              ]
            : [
                {
                  item: {
                    resourceType: "album",
                    resourceId: "virtual-album-1",
                    canonicalId: "fixture:album:shared",
                    title: "Virtual Search Album",
                    subtitle: "Virtual Creator",
                    description:
                      "确定性虚拟搜索结果，包含 verylongunbrokenmetadatavalueforresponsivechecks。",
                  },
                  source: {
                    platformId: "virtual-content",
                    pluginId: longPluginId,
                    pluginName:
                      "Virtual Content Plugin With A Long Responsive Name",
                    pluginVersion: "1.0.0",
                  },
                },
              ],
          sections: [],
          nextCursor: secondPage ? null : "opaque-search-page-2",
          failures:
            options.search === "partial"
              ? [
                  {
                    code: "RESOURCE_ACCESS_DENIED",
                    summary: "Virtual catalog source is unavailable",
                    source: {
                      platformId: "catalog",
                      pluginId: "com.audiodown.catalog.content",
                      pluginName: "Virtual Catalog",
                      pluginVersion: "1.0.0",
                    },
                  },
                ]
              : [],
          emptyState: null,
        });
      }
      return fulfillJson(route, {
        items: [],
        sections: [],
        nextCursor: null,
        failures: [],
        emptyState: {
          reason: "NO_CONTENT_PLUGINS",
          title: "尚未安装内容插件",
          actionLabel: "添加 GitHub 插件仓库",
        },
      });
    }

    if (pathname === "/api/v1/discover") {
      if (method !== "GET") {
        return fulfillJson(
          route,
          { code: "INVALID_DISCOVER_REQUEST", message: "Invalid method" },
          405,
        );
      }
      if (
        options.discover === "results" ||
        options.discover === "partial"
      ) {
        const cursor = url.searchParams.get("cursor");
        if (cursor && cursor !== "opaque-discover-page-2") {
          return fulfillJson(
            route,
            { code: "INVALID_CURSOR", message: "Cursor is invalid" },
            400,
          );
        }
        if (cursor === "opaque-discover-page-2") {
          return fulfillJson(route, {
            items: [],
            sections: [
              {
                section: {
                  id: "more-albums",
                  title: "More Albums",
                  layout: "album-grid",
                  items: [
                    discoverItem(
                      "album",
                      "virtual-album-page-2",
                      "Virtual Discover Album Page Two",
                      "Virtual Page Two Creator",
                    ),
                  ],
                },
                source: discoverSource,
              },
            ],
            nextCursor: null,
            failures: [],
            emptyState: null,
          });
        }
        return fulfillJson(route, {
          items: [],
          sections: discoverSections(),
          nextCursor: "opaque-discover-page-2",
          failures:
            options.discover === "partial"
              ? [
                  {
                    code: "RESOURCE_ACCESS_DENIED",
                    summary:
                      "Virtual catalog discover source is unavailable",
                    source: catalogSource,
                  },
                ]
              : [],
          emptyState: null,
        });
      }
      return fulfillJson(route, {
        items: [],
        sections: [],
        nextCursor: null,
        failures: [],
        emptyState: {
          reason: "NO_CONTENT_PLUGINS",
          title: "尚未安装内容插件",
          actionLabel: "添加 GitHub 插件仓库",
        },
      });
    }

    if (pathname === "/api/v1/categories") {
      if (method !== "GET") {
        return fulfillJson(
          route,
          { code: "INVALID_CATEGORIES_REQUEST", message: "Invalid method" },
          405,
        );
      }
      if (
        options.discover !== "results" &&
        options.discover !== "partial"
      ) {
        return fulfillJson(route, {
          items: [],
          failures: [],
          emptyState: {
            reason: "NO_CONTENT_PLUGINS",
            title: "尚未安装内容插件",
            actionLabel: "添加 GitHub 插件仓库",
          },
        });
      }
      return fulfillJson(route, {
        items: [
          {
            item: {
              resourceId: "virtual-category-1",
              canonicalId: "fixture:category:1",
              title: "Virtual Category",
              description: "Deterministic local category",
            },
            source: discoverSource,
          },
        ],
        failures: [],
        emptyState: null,
      });
    }

    if (pathname === "/api/v1/albums/get") {
      if (method !== "POST") {
        return fulfillJson(
          route,
          { code: "INVALID_ALBUM_REQUEST", message: "Invalid method" },
          405,
        );
      }
      const body = request.postDataJSON() as {
        pluginId?: string;
        resourceId?: string;
      };
      if (
        body.pluginId !== discoverPluginId ||
        body.resourceId === "missing-album"
      ) {
        return fulfillJson(
          route,
          {
            code: "RESOURCE_NOT_FOUND",
            message: "Album resource was not found",
          },
          404,
        );
      }
      if (body.resourceId !== discoverAlbumResourceId) {
        return fulfillJson(
          route,
          {
            code: "RESOURCE_NOT_FOUND",
            message: "Album resource was not found",
          },
          404,
        );
      }
      return fulfillJson(route, {
        album: {
          resourceId: discoverAlbumResourceId,
          canonicalId: "fixture:album:shared",
          title: discoverAlbumTitle,
          creator: "Virtual Primary Creator",
          description:
            "Deterministic local album with verylongunbrokenmetadatavalueforresponsivechecks",
          trackCount: 2,
        },
        source: discoverSource,
      });
    }

    if (pathname === "/api/v1/tracks/list") {
      if (method !== "POST") {
        return fulfillJson(
          route,
          { code: "INVALID_TRACKS_REQUEST", message: "Invalid method" },
          405,
        );
      }
      const body = request.postDataJSON() as {
        pluginId?: string;
        albumResourceId?: string;
        cursor?: string;
      };
      if (
        body.pluginId !== discoverPluginId ||
        body.albumResourceId !== discoverAlbumResourceId
      ) {
        return fulfillJson(
          route,
          {
            code: "RESOURCE_NOT_FOUND",
            message: "Album resource was not found",
          },
          404,
        );
      }
      if (body.cursor && body.cursor !== discoverTrackCursor) {
        return fulfillJson(
          route,
          { code: "INVALID_CURSOR", message: "Cursor is invalid" },
          400,
        );
      }
      const secondPage = body.cursor === discoverTrackCursor;
      return fulfillJson(route, {
        items: [
          {
            resourceId: secondPage
              ? "virtual-track-2"
              : "virtual-track-1",
            canonicalId: secondPage
              ? "fixture:track:2"
              : "fixture:track:1",
            title: secondPage
              ? "Virtual Track 2"
              : "Virtual Track 1",
            sequence: secondPage ? 2 : 1,
            durationSeconds: secondPage ? 125 : 60,
          },
        ],
        source: discoverSource,
        nextCursor: secondPage ? null : discoverTrackCursor,
      });
    }

    if (pathname === "/api/v1/plugins" && method === "GET") {
      return fulfillJson(route, { items: plugins });
    }

    if (pathname === "/api/v1/logs") {
      return fulfillJson(route, {
        items:
          options.logs === "empty"
            ? []
            : [
                {
                  id: "018f0000-0000-7000-8000-000000000001",
                  timestamp: "2026-07-12T08:30:00Z",
                  level: "error",
                  component: "plugin-runtime-with-a-long-component-name",
                  message: longLogMessage,
                  pluginId: longPluginId,
                },
              ],
      });
    }

    if (pathname === "/api/v1/plugin-repositories/inspect") {
      if (options.repositoryInspectionError) {
        return fulfillJson(route, { code: "INSPECTION_FAILED" }, 500);
      }
      return fulfillJson(route, {
        snapshotId: "018f0000-0000-7000-8000-000000000010",
        repository: {
          id: "example.plugins",
          name: "Example Plugin Repository With A Long Name",
          sourceUrl: repositoryUrl,
          commitSha: longCommitSha,
        },
        plugins: [
          {
            pluginId: "com.audiodown.virtual.risk-plugin",
            name:
              options.repositoryRisk ?? true
                ? "Virtual Lifecycle Risk Plugin"
                : "Virtual Content Plugin",
            version: "1.0.0",
            pluginType: "content",
            alreadyInstalled: false,
            requiresLifecycleScriptGrant: options.repositoryRisk ?? true,
            lifecycleScriptReason:
              options.repositoryRisk ?? true
                ? "依赖安装阶段脚本，需要开发者模式下逐次明确授权。"
                : null,
          },
        ],
      });
    }

    if (
      pathname.includes("/plugin-repositories/") &&
      pathname.endsWith("/install")
    ) {
      return fulfillJson(route, plugins[0] ?? {
        pluginId: "com.audiodown.virtual.risk-plugin",
        pluginType: "content",
        platformId: "virtual-content",
        name: "Virtual Lifecycle Risk Plugin",
        version: "1.0.0",
        status: "installed",
        enabled: true,
        runMode: "on_demand",
        priority: 100,
        sourceUrl: repositoryUrl,
        commitSha: longCommitSha,
      });
    }

    const pluginMatch = pathname.match(/^\/api\/v1\/plugins\/([^/]+)$/);
    if (pluginMatch && method === "PATCH") {
      const settings = request.postDataJSON() as {
        enabled: boolean;
        runMode: "on_demand" | "always";
        priority: number;
      };
      plugins = plugins.map((plugin) =>
        plugin.pluginId === decodeURIComponent(pluginMatch[1])
          ? { ...plugin, ...settings }
          : plugin,
      );
      return fulfillJson(route, plugins[0]);
    }
    if (pluginMatch && method === "DELETE") {
      plugins = plugins.filter(
        (plugin) => plugin.pluginId !== decodeURIComponent(pluginMatch[1]),
      );
      return route.fulfill({ status: 204 });
    }

    const runtimeMatch = pathname.match(
      /^\/api\/v1\/plugins\/([^/]+)\/(start|stop)$/,
    );
    if (runtimeMatch && method === "POST") {
      const status = runtimeMatch[2] === "start" ? "running" : "stopped";
      plugins = plugins.map((plugin) =>
        plugin.pluginId === decodeURIComponent(runtimeMatch[1])
          ? { ...plugin, status }
          : plugin,
      );
      return fulfillJson(route, {
        pluginId: decodeURIComponent(runtimeMatch[1]),
        status,
        logs: [],
      });
    }

    return fulfillJson(route, { code: "NOT_FOUND" }, 404);
  });
}
