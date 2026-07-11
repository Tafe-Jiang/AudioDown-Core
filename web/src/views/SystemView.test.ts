import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { flushPromises, mount } from "@vue/test-utils";
import { afterEach, describe, expect, it, vi } from "vitest";

import SystemView from "./SystemView.vue";

const systemResponse = {
  version: "1.0.0-alpha.1",
  supervisor: {
    available: false,
    error:
      "dial unix /private/host/supervisor.sock: connection refused secret-value",
  },
  pluginCount: 3,
  developmentMode: true,
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("system status view", () => {
  it("keeps a stable four-row layout and renders safe system state", async () => {
    let resolveRequest!: (value: unknown) => void;
    vi.stubGlobal(
      "fetch",
      vi.fn().mockReturnValue(
        new Promise((resolve) => {
          resolveRequest = resolve;
        }),
      ),
    );
    const wrapper = mount(SystemView);

    expect(wrapper.findAll("[data-system-row]")).toHaveLength(4);
    expect(
      wrapper.get("[data-system-skeleton]").attributes("aria-busy"),
    ).toBe("true");

    resolveRequest({
      ok: true,
      status: 200,
      json: async () => systemResponse,
    });
    await flushPromises();

    expect(wrapper.findAll("h1")).toHaveLength(1);
    expect(wrapper.text()).toContain("Core 版本");
    expect(wrapper.text()).toContain(systemResponse.version);
    expect(wrapper.text()).toContain("Supervisor");
    expect(wrapper.text()).toContain("不可用");
    expect(wrapper.text()).toContain("已安装插件");
    expect(wrapper.text()).toContain("3");
    expect(wrapper.text()).toContain("开发者模式");
    expect(wrapper.text()).toContain("已启用");
    expect(wrapper.findAll("[data-system-row]")).toHaveLength(4);

    const statuses = wrapper.findAll('[role="status"]');
    expect(statuses.length).toBeGreaterThanOrEqual(2);
    expect(statuses.every((status) => status.find("svg").exists())).toBe(true);
    expect(
      statuses.find((status) => status.text().includes("已启用"))?.attributes(
        "data-tone",
      ),
    ).toBe("warning");

    expect(wrapper.findAll('[role="alert"]')).toHaveLength(1);
    expect(wrapper.get('[role="alert"]').text()).toContain(
      "Supervisor 当前不可用",
    );
    expect(wrapper.text()).not.toContain("/private/host");
    expect(wrapper.text()).not.toContain("secret-value");
  });

  it("contains no fake operations, updates, or secret fields", () => {
    const source = readFileSync(
      resolve(process.cwd(), "src/views/SystemView.vue"),
      "utf8",
    );

    expect(source).not.toContain("<Button");
    expect(source).not.toContain("重启");
    expect(source).not.toContain("自动更新");
    expect(source).not.toContain("developerToken");
    expect(source).not.toContain("开发者令牌");
    expect(source).not.toContain("secret");
    expect(source).not.toContain("token");
  });
});
