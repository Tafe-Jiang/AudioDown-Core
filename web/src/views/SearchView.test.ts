import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import {
  api,
  type ContentEnvelope,
  type PluginItem,
} from "@/api/client";
import SearchView from "./SearchView.vue";

const primary: PluginItem = {
  pluginId: "com.audiodown.virtual.content",
  pluginType: "content",
  platformId: "virtual",
  name: "Virtual Content",
  version: "1.0.0",
  status: "healthy",
  enabled: true,
  runMode: "on_demand",
  priority: 10,
  sourceUrl: "fixture",
  commitSha: "0123456789abcdef0123456789abcdef01234567",
  capabilities: ["content.search"],
  searchEnabled: true,
  discoverEnabled: true,
  isDefaultContentPlugin: true,
};

const catalog: PluginItem = {
  ...primary,
  pluginId: "com.audiodown.catalog.content",
  platformId: "catalog",
  name: "Virtual Catalog",
  isDefaultContentPlugin: false,
};

function envelope(
  overrides: Partial<ContentEnvelope> = {},
): ContentEnvelope {
  return {
    items: [
      {
        item: {
          resourceType: "album",
          resourceId: "virtual-album-1",
          canonicalId: "fixture:album:shared",
          title: "Virtual Aggregated Album",
          subtitle: "Virtual Creator",
          description:
            "A deterministic result with a verylongunbrokenmetadatavalueforresponsivechecks",
        },
        source: {
          platformId: "virtual",
          pluginId: primary.pluginId,
          pluginName: primary.name,
          pluginVersion: primary.version,
        },
      },
    ],
    sections: [],
    nextCursor: "opaque+/=next",
    failures: [
      {
        code: "RESOURCE_ACCESS_DENIED",
        summary: "Virtual catalog source is temporarily unavailable",
        source: {
          platformId: catalog.platformId,
          pluginId: catalog.pluginId,
          pluginName: catalog.name,
          pluginVersion: catalog.version,
        },
      },
    ],
    emptyState: null,
    ...overrides,
  };
}

async function mountView() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/search", component: SearchView },
      { path: "/plugins", component: { template: "<div>插件</div>" } },
    ],
  });
  await router.push("/search");
  await router.isReady();
  return {
    router,
    wrapper: mount(SearchView, {
      global: { plugins: [router] },
    }),
  };
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("search workspace", () => {
  it("loads filters without issuing an empty search and validates locally", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({
      items: [primary, catalog],
    });
    const search = vi.spyOn(api, "search");
    const { wrapper } = await mountView();
    await flushPromises();

    expect(api.plugins).toHaveBeenCalledOnce();
    expect(search).not.toHaveBeenCalled();
    await wrapper.get("form").trigger("submit");
    expect(search).not.toHaveBeenCalled();
    expect(wrapper.get('[role="alert"]').text()).toContain("请输入搜索关键词");
  });

  it("submits platform and plugin filters and keeps successful results beside failures", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({
      items: [primary, catalog],
    });
    vi.spyOn(api, "search").mockResolvedValue(envelope());
    const { wrapper } = await mountView();
    await flushPromises();

    await wrapper.get('input[type="search"]').setValue("fixture");
    await wrapper.get('select[name="platform"]').setValue("virtual");
    await wrapper
      .get('select[name="plugin"]')
      .setValue(primary.pluginId);
    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect(api.search).toHaveBeenCalledWith({
      query: "fixture",
      platformId: "virtual",
      pluginId: primary.pluginId,
      cursor: undefined,
    });
    expect(wrapper.text()).toContain("Virtual Aggregated Album");
    expect(wrapper.text()).toContain("Virtual Content");
    expect(wrapper.text()).toContain("1.0.0");
    expect(wrapper.text()).toContain("部分来源暂不可用");
    expect(wrapper.text()).toContain("RESOURCE_ACCESS_DENIED");
    expect(
      wrapper.text().match(/Virtual Aggregated Album/g) ?? [],
    ).toHaveLength(1);
  });

  it("keeps opaque cursor history for next and previous page requests", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({
      items: [primary, catalog],
    });
    vi.spyOn(api, "search")
      .mockResolvedValueOnce(envelope())
      .mockResolvedValueOnce(
        envelope({
          items: [],
          nextCursor: null,
          failures: [],
        }),
      )
      .mockResolvedValueOnce(envelope());
    const { wrapper } = await mountView();
    await flushPromises();

    await wrapper.get('input[type="search"]').setValue("fixture");
    await wrapper.get("form").trigger("submit");
    await flushPromises();
    await wrapper.get('input[type="search"]').setValue("changed");
    await wrapper.get('select[name="platform"]').setValue("catalog");
    await wrapper.get('button[aria-label="下一页"]').trigger("click");
    await flushPromises();
    expect(api.search).toHaveBeenLastCalledWith({
      query: "fixture",
      platformId: undefined,
      pluginId: undefined,
      cursor: "opaque+/=next",
    });
    await wrapper.get('button[aria-label="上一页"]').trigger("click");
    await flushPromises();
    expect(api.search).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: undefined }),
    );
  });

  it("renders the Core no-plugin empty state", async () => {
    vi.spyOn(api, "plugins").mockResolvedValue({ items: [] });
    vi.spyOn(api, "search").mockResolvedValue(
      envelope({
        items: [],
        nextCursor: null,
        failures: [],
        emptyState: {
          reason: "NO_CONTENT_PLUGINS",
          title: "尚未安装内容插件",
          actionLabel: "添加插件仓库",
        },
      }),
    );
    const { wrapper, router } = await mountView();
    await flushPromises();

    await wrapper.get('input[type="search"]').setValue("fixture");
    await wrapper.get("form").trigger("submit");
    await flushPromises();
    expect(wrapper.text()).toContain("尚未安装内容插件");
    await wrapper
      .findAll("button")
      .find((button) => button.text().includes("添加插件仓库"))!
      .trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/plugins");
  });
});
