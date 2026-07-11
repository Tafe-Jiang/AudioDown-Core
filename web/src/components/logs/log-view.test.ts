import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineComponent, nextTick } from "vue";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import { api, type StructuredLog } from "@/api/client";
import LogsView from "@/views/LogsView.vue";
import LogToolbar from "./LogToolbar.vue";

const infoLog: StructuredLog = {
  id: "018f0000-0000-7000-8000-000000000001",
  timestamp: "2026-07-12T08:30:00Z",
  level: "info",
  component: "core",
  message: "Virtual Alpha installed",
  pluginId: "com.audiodown.virtual.alpha",
};

const errorLog: StructuredLog = {
  id: "018f0000-0000-7000-8000-000000000002",
  timestamp: "2026-07-12T08:31:00Z",
  level: "error",
  component: "plugin-runtime",
  message: "Virtual Beta handshake failed",
  pluginId: "com.audiodown.virtual.beta",
};

const warnLog: StructuredLog = {
  id: "018f0000-0000-7000-8000-000000000003",
  timestamp: "2026-07-12T08:32:00Z",
  level: "warn",
  component: "supervisor",
  message: "Runtime retry scheduled",
  pluginId: null,
};

const passthrough = defineComponent({
  template: "<div><slot /></div>",
});
const conditionalSheet = defineComponent({
  props: ["open"],
  emits: ["update:open"],
  template: '<div v-if="open"><slot /></div>',
});
const sheetStubs = {
  Sheet: conditionalSheet,
  SheetContent: passthrough,
  SheetHeader: passthrough,
  SheetTitle: passthrough,
  SheetDescription: passthrough,
};

function mountLogsView() {
  return mount(LogsView, {
    global: {
      stubs: sheetStubs,
    },
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("structured log workspace", () => {
  it("renders table-shaped skeleton rows while the first request is pending", async () => {
    vi.spyOn(api, "logs").mockReturnValue(new Promise(() => {}));
    const wrapper = mountLogsView();
    await nextTick();

    expect(wrapper.findAll("[data-log-skeleton-row]")).toHaveLength(3);
    expect(wrapper.get("[data-log-skeleton]").text()).toContain("时间");
    expect(wrapper.get("[data-log-skeleton]").text()).toContain("级别");
    expect(wrapper.get("[data-log-skeleton]").text()).toContain("组件");
    expect(wrapper.get("[data-log-skeleton]").text()).toContain("消息");
  });

  it("renders a compact empty state when Core returns no logs", async () => {
    vi.spyOn(api, "logs").mockResolvedValue({ items: [] });
    const wrapper = mountLogsView();
    await flushPromises();

    const empty = wrapper.get("[data-log-empty]");
    expect(empty.text()).toContain("暂无结构化日志");
    expect(empty.classes()).toContain("min-h-40");
  });

  it("filters loaded logs by level, component, and text, then clears all filters", async () => {
    vi.spyOn(api, "logs").mockResolvedValue({
      items: [infoLog, errorLog, warnLog],
    });
    const wrapper = mountLogsView();
    await flushPromises();
    const toolbar = wrapper.findComponent(LogToolbar);

    toolbar.vm.$emit("update:level", "error");
    await nextTick();
    expect(wrapper.text()).toContain(errorLog.message);
    expect(wrapper.text()).not.toContain(infoLog.message);

    toolbar.vm.$emit("update:level", "all");
    toolbar.vm.$emit("update:component", "core");
    await nextTick();
    expect(wrapper.text()).toContain(infoLog.message);
    expect(wrapper.text()).not.toContain(errorLog.message);

    toolbar.vm.$emit("update:component", "all");
    await wrapper.get('input[name="log-query"]').setValue("retry");
    expect(wrapper.text()).toContain(warnLog.message);
    expect(wrapper.text()).not.toContain(infoLog.message);

    await wrapper.get('button[aria-label="清除日志筛选"]').trigger("click");
    expect(wrapper.text()).toContain(infoLog.message);
    expect(wrapper.text()).toContain(errorLog.message);
    expect(wrapper.text()).toContain(warnLog.message);
  });

  it("manually refreshes without clearing filters or sending backend filters", async () => {
    vi.spyOn(api, "logs")
      .mockResolvedValueOnce({ items: [infoLog, errorLog] })
      .mockResolvedValueOnce({ items: [infoLog, errorLog, warnLog] });
    const wrapper = mountLogsView();
    await flushPromises();

    await wrapper.get('input[name="log-query"]').setValue("alpha");
    await wrapper.get('button[aria-label="刷新日志"]').trigger("click");
    await flushPromises();

    expect(api.logs).toHaveBeenCalledTimes(2);
    expect(api.logs).toHaveBeenNthCalledWith(1);
    expect(api.logs).toHaveBeenNthCalledWith(2);
    expect(
      (wrapper.get('input[name="log-query"]').element as HTMLInputElement)
        .value,
    ).toBe("alpha");
    expect(wrapper.text()).toContain(infoLog.message);
    expect(wrapper.text()).not.toContain(warnLog.message);
  });

  it("shows readable desktop and mobile rows with original timestamps", async () => {
    vi.spyOn(api, "logs").mockResolvedValue({ items: [infoLog] });
    const wrapper = mountLogsView();
    await flushPromises();

    const desktop = wrapper.get("[data-desktop-log-table]");
    expect(desktop.text()).toContain("时间");
    expect(desktop.text()).toContain("级别");
    expect(desktop.text()).toContain("组件");
    expect(desktop.text()).toContain("消息");
    expect(desktop.get(`time[datetime="${infoLog.timestamp}"]`)).toBeTruthy();

    const mobile = wrapper.get(`[data-mobile-log="${infoLog.id}"]`);
    expect(mobile.classes()).toContain("min-w-0");
    expect(mobile.find("time").classes()).not.toContain("truncate");
    expect(mobile.get("time").attributes("datetime")).toBe(infoLog.timestamp);
  });

  it("opens a row details sheet using only the Core log fields", async () => {
    vi.spyOn(api, "logs").mockResolvedValue({ items: [errorLog] });
    const wrapper = mountLogsView();
    await flushPromises();

    await wrapper
      .get(`[data-desktop-log-row="${errorLog.id}"]`)
      .trigger("click");
    await nextTick();

    const details = wrapper.get("[data-log-details]");
    expect(details.text()).toContain(errorLog.id);
    expect(details.text()).toContain(errorLog.timestamp);
    expect(details.text()).toContain(errorLog.level);
    expect(details.text()).toContain(errorLog.component);
    expect(details.text()).toContain(errorLog.message);
    expect(details.text()).toContain(errorLog.pluginId!);
  });

  it("keeps the previous successful list visible after a refresh error", async () => {
    vi.spyOn(api, "logs")
      .mockResolvedValueOnce({ items: [infoLog] })
      .mockRejectedValueOnce(new Error("unavailable"));
    const wrapper = mountLogsView();
    await flushPromises();

    await wrapper.get('button[aria-label="刷新日志"]').trigger("click");
    await flushPromises();

    expect(wrapper.text()).toContain("无法读取日志");
    expect(wrapper.text()).toContain(infoLog.message);
  });

  it("does not invent polling, export, deletion, or backend filtering", () => {
    const source = [
      "src/views/LogsView.vue",
      "src/components/logs/LogToolbar.vue",
      "src/components/logs/LogTable.vue",
    ]
      .map((file) => readFileSync(resolve(process.cwd(), file), "utf8"))
      .join("\n");

    expect(source).not.toContain("setInterval");
    expect(source).not.toContain("setTimeout");
    expect(source).not.toContain("导出");
    expect(source).not.toContain("删除日志");
    expect(source).not.toContain("poll");
    expect(source).not.toMatch(/api\.logs\([^)]/);
  });
});
