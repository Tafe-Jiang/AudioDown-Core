import { h, nextTick } from "vue";
import { mount, flushPromises } from "@vue/test-utils";
import {
  createMemoryHistory,
  createRouter,
  type Router,
} from "vue-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import AppShell from "./AppShell.vue";
import { navigation } from "./navigation";
import { useSystemStatus } from "@/composables/useSystemStatus";
import SystemView from "@/views/SystemView.vue";

const systemResponse = {
  version: "1.0.0-alpha.1",
  supervisor: {
    available: true,
    error: null,
  },
  pluginCount: 0,
};

function mockViewport(mobile: boolean) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn().mockImplementation((query: string) => ({
      matches: mobile && query === "(max-width: 767px)",
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  );
}

async function createTestRouter(path = "/plugins"): Promise<Router> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: navigation.map((item) => ({
      path: item.to,
      component: { template: "<div />" },
      meta: { title: item.label },
    })),
  });
  await router.push(path);
  await router.isReady();
  return router;
}

beforeEach(() => {
  localStorage.clear();
  document.body.innerHTML = "";
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("responsive application shell", () => {
  it("shares the first system request between concurrent consumers", async () => {
    const fetchMock = vi.fn().mockResolvedValue({
      ok: true,
      json: async () => systemResponse,
    });
    vi.stubGlobal("fetch", fetchMock);

    const first = useSystemStatus();
    const second = useSystemStatus();
    await flushPromises();

    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(first.system.value).toEqual(systemResponse);
    expect(second.system.value).toBe(first.system.value);
  });

  it("renders five icon routes, current title, and stable desktop collapse", async () => {
    mockViewport(false);
    const router = await createTestRouter();
    const wrapper = mount(AppShell, {
      attachTo: document.body,
      global: { plugins: [router] },
      slots: {
        default: () => h(SystemView),
      },
    });
    await flushPromises();

    const links = wrapper.findAll("[data-navigation-link]");
    expect(links).toHaveLength(5);
    expect(links.every((link) => link.find("svg").exists())).toBe(true);
    expect(
      wrapper.get('[data-navigation-link="/plugins"]').attributes(
        "aria-current",
      ),
    ).toBe("page");
    expect(wrapper.get('[data-slot="app-header"]').text()).toContain("插件");
    expect(wrapper.text()).toContain("Supervisor 可用");
    expect(wrapper.text()).not.toMatch(/\b[DSPIL]\b/);

    const trigger = wrapper.get('[data-slot="sidebar-trigger"]');
    expect(trigger.attributes("aria-label")).toBe("折叠侧栏");
    await trigger.trigger("click");
    await nextTick();

    expect(localStorage.getItem("audiodown.sidebar.collapsed")).toBe("true");
    expect(
      wrapper.get('[data-slot="sidebar"][data-state]').attributes("data-state"),
    ).toBe("collapsed");
    expect(
      wrapper
        .findAll("[data-navigation-link]")
        .every((link) => Boolean(link.attributes("aria-label"))),
    ).toBe(true);
  });

  it("opens mobile navigation as a sheet and closes it after navigation", async () => {
    mockViewport(true);
    const router = await createTestRouter("/discover");
    const wrapper = mount(AppShell, {
      attachTo: document.body,
      global: { plugins: [router] },
      slots: {
        default: '<button data-page-content>页面操作</button>',
      },
    });
    await flushPromises();

    const trigger = wrapper.get('[data-slot="sidebar-trigger"]');
    expect(trigger.attributes("aria-label")).toBe("打开主导航");
    await trigger.trigger("click");
    await nextTick();

    const sheet = document.querySelector('[data-mobile="true"]');
    expect(sheet).not.toBeNull();
    const searchLink = sheet?.querySelector(
      '[data-navigation-link="/search"]',
    ) as HTMLElement;
    searchLink.click();
    await flushPromises();

    expect(router.currentRoute.value.path).toBe("/search");
    expect(document.querySelector('[data-mobile="true"]')).toBeNull();
  });
});
