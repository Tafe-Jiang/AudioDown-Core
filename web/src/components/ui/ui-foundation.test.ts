import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { mount } from "@vue/test-utils";
import { expect, it } from "vitest";

import Button from "@/components/ui/button/Button.vue";
import Empty from "@/components/ui/empty/Empty.vue";
import Table from "@/components/ui/table/Table.vue";

it("provides Vue primitives without React dependencies", () => {
  const packageJson = JSON.parse(
    readFileSync(resolve(process.cwd(), "package.json"), "utf8"),
  );

  expect(packageJson.dependencies).not.toHaveProperty("react");
  expect(packageJson.dependencies).not.toHaveProperty("lucide-react");
  expect(packageJson.dependencies).not.toHaveProperty("@tanstack/vue-table");
  expect(mount(Button, { slots: { default: "保存" } }).text()).toBe("保存");
  expect(mount(Empty).exists()).toBe(true);
  expect(mount(Table).element.tagName).toBe("TABLE");
});

it("does not animate disabled opacity through low-contrast frames", () => {
  const button = mount(Button, { slots: { default: "搜索" } });

  expect(button.classes()).toContain("transition-colors");
  expect(button.classes()).not.toContain("transition-all");
});
