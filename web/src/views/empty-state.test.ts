import { flushPromises, mount } from "@vue/test-utils";
import { createMemoryHistory, createRouter } from "vue-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import DiscoverView from "./DiscoverView.vue";
import PluginsView from "./PluginsView.vue";
import SearchView from "./SearchView.vue";

const emptyState = {
  reason: "NO_CONTENT_PLUGINS",
  title: "尚未安装内容插件",
  actionLabel: "添加 GitHub 插件仓库",
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("empty state views", () => {
  it.each([
    ["Discover", DiscoverView],
    ["Search", SearchView],
  ])("renders the %s empty state from Core", async (_name, view) => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        json: async () => emptyState,
      }),
    );

    const router = createRouter({
      history: createMemoryHistory(),
      routes: [
        { path: "/", component: view },
        {
          path: "/plugins",
          component: { template: "<div>插件页</div>" },
        },
      ],
    });
    await router.push("/");
    await router.isReady();
    const wrapper = mount(view, {
      global: {
        plugins: [router],
      },
    });
    await flushPromises();

    expect(wrapper.text()).toContain("尚未安装内容插件");
    expect(wrapper.text()).toContain("添加 GitHub 插件仓库");
    expect(wrapper.find(".empty-signal").exists()).toBe(false);
    expect(wrapper.find(".loading-line").exists()).toBe(false);
  });

  it("shows Supervisor availability without platform labels", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn((url: string) =>
        Promise.resolve({
          ok: true,
          json: async () =>
            url.endsWith("/system")
              ? {
                  version: "1.0.0-alpha.1",
                  supervisor: {
                    available: false,
                    error: "Supervisor is unavailable",
                  },
                  pluginCount: 0,
                }
              : { items: [] },
        }),
      ),
    );

    const wrapper = mount(PluginsView);
    await flushPromises();

    expect(wrapper.text()).toContain("Supervisor 不可用");
    expect(wrapper.text().toLowerCase()).not.toContain(
      "hardcoded-platform-label",
    );
  });
});
