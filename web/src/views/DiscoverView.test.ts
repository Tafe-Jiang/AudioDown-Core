import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  api,
  type ContentEnvelope,
  type ContentSource,
  type PluginItem,
} from "@/api/client";
import DiscoverView from "./DiscoverView.vue";

const source: ContentSource = {
  platformId: "virtual",
  pluginId: "com.audiodown.virtual.content",
  pluginName: "Virtual Content",
  pluginVersion: "1.0.0",
};

const plugin: PluginItem = {
  pluginId: source.pluginId,
  pluginType: "content",
  platformId: source.platformId,
  name: source.pluginName,
  version: source.pluginVersion,
  status: "healthy",
  enabled: true,
  runMode: "on_demand",
  priority: 10,
  sourceUrl: "fixture",
  commitSha: "0123456789abcdef0123456789abcdef01234567",
  capabilities: [
    "content.discover",
    "content.categories",
    "content.album.get",
    "content.tracks.list",
  ],
  searchEnabled: true,
  discoverEnabled: true,
  isDefaultContentPlugin: true,
};

const catalogPlugin: PluginItem = {
  ...plugin,
  pluginId: "com.audiodown.catalog.content",
  platformId: "catalog",
  name: "Virtual Catalog",
  isDefaultContentPlugin: false,
};

function item(
  title: string,
  resourceId: string,
  resourceType: "album" | "track" | "category" = "album",
) {
  return {
    resourceType,
    resourceId,
    canonicalId: `fixture:${resourceType}:${resourceId}`,
    title,
    subtitle: resourceType === "album" ? "Virtual Creator" : undefined,
    description: "Deterministic local content",
  };
}

function discoverEnvelope(
  overrides: Partial<ContentEnvelope> = {},
): ContentEnvelope {
  return {
    items: [],
    sections: [
      {
        section: {
          id: "hero",
          title: "Featured",
          layout: "hero-carousel",
          items: [item("Virtual Hero Album", "virtual-album-hero")],
        },
        source,
      },
      {
        section: {
          id: "albums",
          title: "Albums",
          layout: "album-grid",
          items: [item("Virtual Grid Album", "virtual-album-grid")],
        },
        source,
      },
      {
        section: {
          id: "recent",
          title: "Recent",
          layout: "horizontal-list",
          items: [item("Virtual Recent Album", "virtual-album-recent")],
        },
        source,
      },
      {
        section: {
          id: "ranked",
          title: "Ranked",
          layout: "ranked-list",
          items: [item("Virtual Ranked Album", "virtual-album-ranked")],
        },
        source,
      },
      {
        section: {
          id: "categories",
          title: "Categories",
          layout: "category-grid",
          items: [
            item(
              "Virtual Discover Category",
              "virtual-category-discover",
              "category",
            ),
          ],
        },
        source,
      },
    ],
    nextCursor: "opaque-discover-page-2",
    failures: [
      {
        code: "RESOURCE_ACCESS_DENIED",
        summary: "Virtual catalog discover source is unavailable",
        source: {
          platformId: catalogPlugin.platformId,
          pluginId: catalogPlugin.pluginId,
          pluginName: catalogPlugin.name,
          pluginVersion: catalogPlugin.version,
        },
      },
    ],
    emptyState: null,
    ...overrides,
  };
}

function categoriesResponse() {
  return {
    items: [
      {
        item: {
          resourceId: "virtual-category-1",
          canonicalId: "fixture:category:1",
          title: "Virtual Category",
          description: "Deterministic local category",
        },
        source,
      },
    ],
    failures: [],
    emptyState: null,
  };
}

async function mountView() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/discover", component: DiscoverView },
      {
        path: "/albums/detail",
        name: "album",
        component: { template: "<div>Album route</div>" },
      },
      { path: "/plugins", component: { template: "<div>Plugins</div>" } },
    ],
  });
  await router.push("/discover");
  await router.isReady();
  return {
    router,
    wrapper: mount(DiscoverView, {
      global: { plugins: [router] },
    }),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("discover workspace", () => {
  it("renders categories and every standard discover layout", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({
      items: [plugin, catalogPlugin],
    });
    vi.spyOn(api, "discover").mockResolvedValue(discoverEnvelope());
    vi.spyOn(api, "categories").mockResolvedValue(
      categoriesResponse() as never,
    );

    const { wrapper } = await mountView();
    await flushPromises();

    expect(api.discover).toHaveBeenCalledWith({
      platformId: undefined,
      pluginId: undefined,
      cursor: undefined,
    });
    expect(api.categories).toHaveBeenCalledWith({
      platformId: undefined,
      pluginId: undefined,
    });
    expect(wrapper.text()).toContain("Virtual Category");
    expect(wrapper.text()).toContain("Virtual Content");
    expect(wrapper.text()).toContain("1.0.0");
    expect(
      wrapper
        .findAll("[data-discover-layout]")
        .map((section) => section.attributes("data-discover-layout")),
    ).toEqual([
      "hero-carousel",
      "album-grid",
      "horizontal-list",
      "ranked-list",
      "category-grid",
    ]);
    expect(wrapper.text()).toContain("部分来源暂不可用");
  });

  it("applies platform and plugin filters and keeps cursors opaque", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({
      items: [plugin, catalogPlugin],
    });
    vi.spyOn(api, "discover")
      .mockResolvedValueOnce(discoverEnvelope())
      .mockResolvedValueOnce(discoverEnvelope())
      .mockResolvedValueOnce(
        discoverEnvelope({
          sections: [],
          nextCursor: null,
          failures: [],
        }),
      )
      .mockResolvedValueOnce(discoverEnvelope());
    vi.spyOn(api, "categories").mockResolvedValue(
      categoriesResponse() as never,
    );

    const { wrapper } = await mountView();
    await flushPromises();
    await wrapper.get('select[name="platform"]').setValue("virtual");
    await wrapper.get('select[name="plugin"]').setValue(plugin.pluginId);
    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect(api.discover).toHaveBeenLastCalledWith({
      platformId: "virtual",
      pluginId: plugin.pluginId,
      cursor: undefined,
    });
    expect(api.categories).toHaveBeenLastCalledWith({
      platformId: "virtual",
      pluginId: plugin.pluginId,
    });

    await wrapper.get('select[name="platform"]').setValue("catalog");
    await wrapper.get('button[aria-label="下一页"]').trigger("click");
    await flushPromises();
    expect(api.discover).toHaveBeenLastCalledWith({
      platformId: "virtual",
      pluginId: plugin.pluginId,
      cursor: "opaque-discover-page-2",
    });

    await wrapper.get('button[aria-label="上一页"]').trigger("click");
    await flushPromises();
    expect(api.discover).toHaveBeenLastCalledWith({
      platformId: "virtual",
      pluginId: plugin.pluginId,
      cursor: undefined,
    });
  });

  it("opens albums with source plugin and opaque resource identity", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({ items: [plugin] });
    vi.spyOn(api, "discover").mockResolvedValue(discoverEnvelope());
    vi.spyOn(api, "categories").mockResolvedValue(
      categoriesResponse() as never,
    );

    const { wrapper, router } = await mountView();
    await flushPromises();
    await wrapper
      .get('[data-resource-id="virtual-album-hero"]')
      .trigger("click");
    await flushPromises();

    expect(router.currentRoute.value.name).toBe("album");
    expect(router.currentRoute.value.query).toEqual({
      pluginId: plugin.pluginId,
      resourceId: "virtual-album-hero",
    });
  });

  it("renders the Core no-plugin empty state without invented content", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({ items: [] });
    vi.spyOn(api, "discover").mockResolvedValue(
      discoverEnvelope({
        sections: [],
        nextCursor: null,
        failures: [],
        emptyState: {
          reason: "NO_CONTENT_PLUGINS",
          title: "尚未安装内容插件",
          actionLabel: "添加 GitHub 插件仓库",
        },
      }),
    );
    vi.spyOn(api, "categories").mockResolvedValue({
      items: [],
      failures: [],
      emptyState: {
        reason: "NO_CONTENT_PLUGINS",
        title: "尚未安装内容插件",
        actionLabel: "添加 GitHub 插件仓库",
      },
    });

    const { wrapper } = await mountView();
    await flushPromises();

    expect(wrapper.text()).toContain("尚未安装内容插件");
    expect(wrapper.findAll("[data-discover-layout]")).toHaveLength(0);
  });
});
