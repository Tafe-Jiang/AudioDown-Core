import { defineComponent, nextTick } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import { api, type PluginItem } from "@/api/client";
import PluginActionsMenu from "./PluginActionsMenu.vue";
import PluginSettingsSheet from "./PluginSettingsSheet.vue";
import PluginTable from "./PluginTable.vue";

const alpha: PluginItem = {
  pluginId: "com.audiodown.virtual.alpha",
  pluginType: "content",
  platformId: "virtual-alpha",
  name: "Virtual Alpha",
  version: "1.2.0",
  status: "installed",
  enabled: true,
  runMode: "on_demand",
  priority: 25,
  sourceUrl: "https://github.com/example/virtual-alpha",
  commitSha: "0123456789abcdef0123456789abcdef01234567",
  capabilities: [
    "content.search",
    "content.discover",
    "content.categories",
  ],
  searchEnabled: true,
  discoverEnabled: false,
  isDefaultContentPlugin: false,
};

const beta: PluginItem = {
  ...alpha,
  pluginId: "com.audiodown.virtual.beta",
  pluginType: "credential",
  platformId: "virtual-beta",
  name: "Virtual Beta",
  version: "2.0.0",
  status: "running",
  runMode: "always",
  priority: 80,
  capabilities: [],
  searchEnabled: null,
  discoverEnabled: null,
  isDefaultContentPlugin: false,
};

const passthrough = defineComponent({
  template: "<div><slot /></div>",
});
const buttonStub = defineComponent({
  emits: ["select"],
  template: '<button type="button" @click="$emit(\'select\')"><slot /></button>',
});
const selectStub = defineComponent({
  name: "Select",
  props: ["modelValue", "disabled"],
  emits: ["update:modelValue"],
  template: '<div data-select><slot /></div>',
});
const selectItemStub = defineComponent({
  props: ["value"],
  template: "<div><slot /></div>",
});

const primitiveStubs = {
  DropdownMenu: passthrough,
  DropdownMenuTrigger: passthrough,
  DropdownMenuContent: passthrough,
  DropdownMenuItem: buttonStub,
  DropdownMenuSeparator: passthrough,
  TooltipProvider: passthrough,
  Tooltip: passthrough,
  TooltipTrigger: passthrough,
  TooltipContent: passthrough,
  Sheet: passthrough,
  SheetContent: passthrough,
  SheetHeader: passthrough,
  SheetTitle: passthrough,
  SheetDescription: passthrough,
  SheetFooter: passthrough,
  Select: selectStub,
  SelectTrigger: passthrough,
  SelectValue: passthrough,
  SelectContent: passthrough,
  SelectItem: selectItemStub,
  AlertDialog: passthrough,
  AlertDialogContent: passthrough,
  AlertDialogHeader: passthrough,
  AlertDialogTitle: passthrough,
  AlertDialogDescription: passthrough,
  AlertDialogFooter: passthrough,
  AlertDialogCancel: passthrough,
  AlertDialogAction: buttonStub,
};

function mountTable(supervisorAvailable = true) {
  return mount(PluginTable, {
    props: {
      items: [alpha, beta],
      supervisorAvailable,
    },
    global: {
      stubs: primitiveStubs,
    },
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("installed plugin management", () => {
  it("renders a semantic desktop table and stable mobile items", () => {
    const wrapper = mountTable();
    const desktop = wrapper.get("[data-desktop-plugin-table]");

    expect(desktop.text()).toContain("名称");
    expect(desktop.text()).toContain("类型");
    expect(desktop.text()).toContain("版本");
    expect(desktop.text()).toContain("状态");
    expect(desktop.text()).toContain("运行模式");
    expect(desktop.text()).toContain("优先级");
    expect(desktop.text()).toContain("操作");
    expect(desktop.get("table").element.tagName).toBe("TABLE");

    const status = desktop.findAll('[role="status"]')[0];
    expect(status.text()).toContain("已安装");
    expect(status.find("svg").exists()).toBe(true);

    const mobile = wrapper.get(
      `[data-mobile-plugin-item="${alpha.pluginId}"]`,
    );
    expect(mobile.text()).toContain(alpha.name);
    expect(mobile.text()).toContain("按需运行");
    expect(mobile.classes()).toContain("min-w-0");
    expect(mobile.classes()).toContain("md:hidden");
    expect(mobile.classes()).not.toContain("overflow-x-auto");
  });

  it("rolls an enable switch back and attaches the API error to one plugin", async () => {
    vi.spyOn(api, "updatePlugin").mockRejectedValue(new Error("failed"));
    const wrapper = mountTable();
    const toggle = wrapper.get(
      `[data-plugin-enable="${alpha.pluginId}"]`,
    );

    expect(toggle.attributes("aria-checked")).toBe("true");
    await toggle.trigger("click");
    await flushPromises();

    expect(api.updatePlugin).toHaveBeenCalledWith(alpha.pluginId, {
      enabled: false,
      runMode: alpha.runMode,
      priority: alpha.priority,
    });
    expect(toggle.attributes("aria-checked")).toBe("true");
    expect(
      wrapper.get(`[data-plugin-error="${alpha.pluginId}"]`).text(),
    ).toContain("更新插件失败");
    expect(
      wrapper.find(`[data-plugin-error="${beta.pluginId}"]`).exists(),
    ).toBe(false);
  });

  it("refreshes authoritative items after a successful change", async () => {
    const updated = { ...alpha, enabled: false };
    vi.spyOn(api, "updatePlugin").mockResolvedValue(updated);
    vi.spyOn(api, "plugins").mockResolvedValue({ items: [updated, beta] });
    const wrapper = mountTable();

    await wrapper
      .get(`[data-plugin-enable="${alpha.pluginId}"]`)
      .trigger("click");
    await flushPromises();

    expect(api.plugins).toHaveBeenCalledOnce();
    expect(wrapper.emitted("items-refreshed")?.at(-1)).toEqual([
      [updated, beta],
    ]);
  });

  it("keeps busy and error state isolated to the affected plugin", async () => {
    let resolveStart!: (value: {
      pluginId: string;
      status: string;
      logs: never[];
    }) => void;
    vi.spyOn(api, "startPlugin").mockReturnValue(
      new Promise((resolve) => {
        resolveStart = resolve;
      }),
    );
    vi.spyOn(api, "plugins").mockResolvedValue({
      items: [{ ...alpha, status: "running" }, beta],
    });
    const wrapper = mountTable();

    await wrapper.get(`button[aria-label="启动 ${alpha.name}"]`).trigger("click");
    await nextTick();

    expect(
      wrapper.get(`button[aria-label="启动 ${alpha.name}"]`).attributes(),
    ).toHaveProperty("disabled");
    expect(
      wrapper.get(`button[aria-label="停止 ${beta.name}"]`).attributes(),
    ).not.toHaveProperty("disabled");

    resolveStart({
      pluginId: alpha.pluginId,
      status: "running",
      logs: [],
    });
    await flushPromises();
    expect(wrapper.emitted("items-refreshed")).toBeTruthy();
  });

  it("disables runtime-changing controls when Supervisor is unavailable", () => {
    const wrapper = mountTable(false);

    expect(
      wrapper.get(`button[aria-label="启动 ${alpha.name}"]`).attributes(),
    ).toHaveProperty("disabled");
    expect(
      wrapper.get(`button[aria-label="停止 ${beta.name}"]`).attributes(),
    ).toHaveProperty("disabled");
    expect(
      wrapper
        .get(`[data-plugin-enable="${alpha.pluginId}"]`)
        .attributes("data-disabled"),
    ).toBe("");
  });

  it("edits runtime and content routing in a named settings sheet", async () => {
    const wrapper = mount(PluginSettingsSheet, {
      props: {
        open: true,
        plugin: alpha,
        platformPlugins: [alpha],
        busy: false,
        error: "",
      },
      global: {
        stubs: primitiveStubs,
      },
    });

    expect(wrapper.text()).toContain(`设置 ${alpha.name}`);
    const selects = wrapper.findAllComponents(selectStub);
    selects[0].vm.$emit("update:modelValue", "always");
    selects[1].vm.$emit("update:modelValue", alpha.pluginId);
    await nextTick();
    await wrapper.get('input[name="priority"]').setValue(7);
    await wrapper
      .get('[data-content-setting="search"]')
      .trigger("click");
    await wrapper
      .get('[data-content-setting="discover"]')
      .trigger("click");
    await wrapper.get("form").trigger("submit");

    expect(wrapper.emitted("save")?.[0]).toEqual([
      {
        enabled: alpha.enabled,
        runMode: "always",
        priority: 7,
        searchEnabled: false,
        discoverEnabled: true,
        defaultContentPluginId: alpha.pluginId,
      },
    ]);
    expect(wrapper.text()).toContain("content.search");
    expect(wrapper.text()).toContain("默认内容插件");
  });

  it("uses labeled icon commands and requests named destructive actions", async () => {
    const startMenu = mount(PluginActionsMenu, {
      props: {
        plugin: alpha,
        busy: false,
        supervisorAvailable: true,
      },
      global: { stubs: primitiveStubs },
    });
    expect(
      startMenu.get(`button[aria-label="启动 ${alpha.name}"]`).attributes(
        "title",
      ),
    ).toBe(`启动 ${alpha.name}`);

    const stopMenu = mount(PluginActionsMenu, {
      props: {
        plugin: beta,
        busy: false,
        supervisorAvailable: true,
      },
      global: { stubs: primitiveStubs },
    });
    expect(
      stopMenu.get(`button[aria-label="停止 ${beta.name}"]`).attributes(
        "title",
      ),
    ).toBe(`停止 ${beta.name}`);

    await startMenu.get('[data-action="settings"]').trigger("click");
    await startMenu.get('[data-action="uninstall"]').trigger("click");
    expect(startMenu.emitted("settings")).toBeTruthy();
    expect(startMenu.emitted("uninstall")).toBeTruthy();
    expect(startMenu.text()).toContain("卸载");
  });

  it("names the plugin in destructive confirmation and refreshes after uninstall", async () => {
    vi.spyOn(api, "uninstallPlugin").mockResolvedValue();
    vi.spyOn(api, "plugins").mockResolvedValue({ items: [beta] });
    const wrapper = mountTable();

    wrapper
      .findAllComponents(PluginActionsMenu)[0]
      .vm.$emit("uninstall");
    await nextTick();

    expect(wrapper.text()).toContain(`卸载 ${alpha.name}？`);
    const confirm = wrapper
      .findAll("button")
      .find((button) => button.text().includes("确认卸载"));
    expect(confirm).toBeTruthy();
    await confirm!.trigger("click");
    await flushPromises();

    expect(api.uninstallPlugin).toHaveBeenCalledWith(alpha.pluginId);
    expect(wrapper.emitted("items-refreshed")?.at(-1)).toEqual([[beta]]);
  });
});
