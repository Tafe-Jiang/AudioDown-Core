import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import AsyncState from "./AsyncState.vue";
import EmptyState from "./EmptyState.vue";
import PageHeader from "./PageHeader.vue";
import ResponsiveDialog from "./ResponsiveDialog.vue";
import StatusBadge from "./StatusBadge.vue";
import Dialog from "@/components/ui/dialog/Dialog.vue";
import Drawer from "@/components/ui/drawer/Drawer.vue";

function mockViewport(mobile: boolean) {
  vi.stubGlobal(
    "matchMedia",
    vi.fn().mockImplementation((query: string) => ({
      matches: mobile && query === "(max-width: 760px)",
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

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("shared workspace components", () => {
  it("renders a compact page title and optional action", () => {
    const wrapper = mount(PageHeader, {
      props: {
        title: "插件",
        description: "管理已安装插件",
      },
      slots: {
        actions: "<button>添加仓库</button>",
      },
    });

    expect(wrapper.get("h1").text()).toBe("插件");
    expect(wrapper.text()).toContain("管理已安装插件");
    expect(wrapper.get("button").text()).toBe("添加仓库");
  });

  it.each([
    ["success", "Supervisor 可用"],
    ["warning", "Supervisor 降级"],
    ["danger", "Supervisor 不可用"],
    ["neutral", "状态未知"],
  ] as const)("maps %s status to text plus an icon", (tone, label) => {
    const wrapper = mount(StatusBadge, {
      props: { tone, label },
    });

    expect(wrapper.get('[role="status"]').text()).toContain(label);
    expect(wrapper.get('[role="status"]').attributes("data-tone")).toBe(tone);
    expect(wrapper.find("svg").exists()).toBe(true);
  });

  it("reserves loading layout and exposes an accessible retry command", async () => {
    const loading = mount(AsyncState, {
      props: { loading: true },
    });
    expect(loading.get('[role="status"]').attributes("aria-label")).toBe(
      "加载中",
    );
    expect(loading.findAll('[data-slot="skeleton"]').length).toBeGreaterThan(1);

    const failed = mount(AsyncState, {
      props: {
        loading: false,
        error: "Core 暂时不可用",
      },
    });
    expect(failed.get('[role="alert"]').text()).toContain("Core 暂时不可用");
    expect(failed.get("button").attributes("aria-label")).toBe("重试");
    await failed.get("button").trigger("click");
    expect(failed.emitted("retry")).toHaveLength(1);
  });

  it("supports primary and optional secondary empty-state commands", async () => {
    const wrapper = mount(EmptyState, {
      props: {
        title: "暂无插件",
        description: "添加仓库后可在这里管理插件。",
        primaryLabel: "添加仓库",
        secondaryLabel: "查看日志",
      },
    });

    expect(wrapper.text()).toContain("暂无插件");
    expect(wrapper.findAll("button")).toHaveLength(2);
    await wrapper.findAll("button")[0].trigger("click");
    await wrapper.findAll("button")[1].trigger("click");
    expect(wrapper.emitted("primary")).toHaveLength(1);
    expect(wrapper.emitted("secondary")).toHaveLength(1);
  });

  it("uses Dialog on desktop and Drawer on mobile", () => {
    mockViewport(false);
    const desktop = mount(ResponsiveDialog, {
      props: {
        open: true,
        title: "插件设置",
        description: "修改插件运行策略",
      },
    });
    expect(desktop.findComponent(Dialog).exists()).toBe(true);
    expect(desktop.findComponent(Drawer).exists()).toBe(false);

    desktop.unmount();
    vi.unstubAllGlobals();
    mockViewport(true);
    const mobile = mount(ResponsiveDialog, {
      props: {
        open: true,
        title: "插件设置",
        description: "修改插件运行策略",
      },
    });
    expect(mobile.findComponent(Drawer).exists()).toBe(true);
    expect(mobile.findComponent(Dialog).exists()).toBe(false);
  });

  it("mounts one toast container at the application root", () => {
    const source = readFileSync(
      resolve(process.cwd(), "src/main.ts"),
      "utf8",
    );

    expect(source.match(/h\(Toaster/g)).toHaveLength(1);
  });
});
