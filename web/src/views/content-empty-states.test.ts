import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import DiscoverView from "./DiscoverView.vue";
import SearchView from "./SearchView.vue";

const emptyState = {
  reason: "NO_CONTENT_PLUGINS",
  title: "尚未安装内容插件",
  actionLabel: "添加 GitHub 插件仓库",
};
const emptyEnvelope = {
  items: [],
  sections: [],
  nextCursor: null,
  failures: [],
  emptyState,
};

async function mountWithRouter(component: typeof DiscoverView) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: "/discover", component: DiscoverView },
      { path: "/search", component: SearchView },
      { path: "/plugins", component: { template: "<div>插件页</div>" } },
    ],
  });
  await router.push(component === DiscoverView ? "/discover" : "/search");
  await router.isReady();
  const wrapper = mount(component, {
    global: { plugins: [router] },
  });
  return { wrapper, router };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("content capability empty workflows", () => {
  it("renders one unframed discover empty state with a Compass icon", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => emptyState,
      }),
    );
    const { wrapper, router } = await mountWithRouter(DiscoverView);
    await flushPromises();

    const empty = wrapper.findAll('[data-slot="empty"]');
    expect(empty).toHaveLength(1);
    expect(empty[0].classes()).toContain("border-0");
    expect(empty[0].find("svg").exists()).toBe(true);
    expect(wrapper.text()).toContain(emptyState.title);
    await wrapper
      .findAll("button")
      .find((button) => button.text().includes(emptyState.actionLabel))!
      .trigger("click");
    await flushPromises();
    expect(router.currentRoute.value.path).toBe("/plugins");
  });

  it("keeps the search form visible and submits icon plus accessible text", async () => {
    const fetchMock = vi.fn((url: string) =>
      Promise.resolve({
        ok: true,
        json: async () =>
          url.endsWith("/plugins") ? { items: [] } : emptyEnvelope,
      }),
    );
    vi.stubGlobal("fetch", fetchMock);
    const { wrapper } = await mountWithRouter(SearchView);
    await flushPromises();

    const input = wrapper.get('input[type="search"]');
    await input.setValue("本地测试");
    const button = wrapper.get('button[type="submit"]');
    expect(button.text()).toContain("搜索");
    expect(button.find("svg").exists()).toBe(true);
    await wrapper.get("form").trigger("submit");
    await flushPromises();

    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/search?q=%E6%9C%AC%E5%9C%B0%E6%B5%8B%E8%AF%95",
      expect.anything(),
    );
    expect(
      (wrapper.get('input[type="search"]').element as HTMLInputElement).value,
    ).toBe("本地测试");
    expect(wrapper.text()).toContain(emptyState.title);
  });

  it("uses stable loading skeletons", async () => {
    vi.stubGlobal("fetch", vi.fn().mockReturnValue(new Promise(() => {})));
    const { wrapper } = await mountWithRouter(DiscoverView);

    expect(wrapper.findAll('[data-slot="skeleton"]').length).toBeGreaterThan(1);
    expect(wrapper.text()).not.toContain("正在读取 Core 状态");
  });

  it("shows retryable search errors without hiding the query", async () => {
    let searchAttempts = 0;
    const fetchMock = vi.fn((url: string) => {
      if (url.endsWith("/plugins")) {
        return Promise.resolve({
          ok: true,
          json: async () => ({ items: [] }),
        });
      }
      searchAttempts += 1;
      if (searchAttempts === 1) {
        return Promise.reject(new Error("offline"));
      }
      return Promise.resolve({
        ok: true,
        json: async () => emptyEnvelope,
      });
    });
    vi.stubGlobal("fetch", fetchMock);
    const { wrapper } = await mountWithRouter(SearchView);
    await flushPromises();

    const input = wrapper.get('input[type="search"]');
    await input.setValue("保留查询");
    await wrapper.get("form").trigger("submit");
    await flushPromises();
    expect(wrapper.get('[role="alert"]').text()).toContain("无法读取搜索结果");
    await wrapper.get('button[aria-label="重试"]').trigger("click");
    await flushPromises();

    expect(
      (wrapper.get('input[type="search"]').element as HTMLInputElement).value,
    ).toBe("保留查询");
    expect(wrapper.text()).toContain(emptyState.title);
  });

  it("does not invent content or repository data", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve({
          ok: true,
          json: async () =>
            url.endsWith("/plugins") ? { items: [] } : emptyEnvelope,
        }),
      ),
    );
    const { wrapper } = await mountWithRouter(SearchView);
    await flushPromises();

    const text = wrapper.text().toLowerCase();
    for (const forbidden of [
      "album",
      "track",
      "chart",
      "platform",
      "repository.example",
    ]) {
      expect(text).not.toContain(forbidden);
    }
  });
});
