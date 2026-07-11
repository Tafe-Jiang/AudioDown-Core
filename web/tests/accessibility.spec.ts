import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

import { mockCoreApi } from "./fixtures/mock-api";

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "tablet", width: 1024, height: 768 },
  { name: "mobile", width: 390, height: 844 },
] as const;
const routes = ["/discover", "/search", "/plugins", "/logs", "/system"];

for (const viewport of viewports) {
  test(`${viewport.name} routes pass structural accessibility checks`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await mockCoreApi(page);

    for (const path of routes) {
      await page.goto(path);
      await expect(page.locator("h1")).toHaveCount(1);

      if (viewport.width < 768) {
        await page.getByRole("button", { name: "打开主导航" }).click();
      }
      const navigation = page.getByRole("navigation", { name: "主导航" });
      await expect(navigation).toBeVisible();
      await expect(navigation.locator('[aria-current="page"]')).toHaveCount(1);
      if (viewport.width < 768) {
        await page.keyboard.press("Escape");
      }

      const iconButtons = page.locator("button:visible").filter({
        has: page.locator("svg"),
      });
      for (let index = 0; index < (await iconButtons.count()); index += 1) {
        const button = iconButtons.nth(index);
        const name =
          (await button.getAttribute("aria-label")) ??
          (await button.textContent())?.trim();
        expect(name).toBeTruthy();
      }

      const results = await new AxeBuilder({ page }).analyze();
      expect(
        results.violations.filter((violation) =>
          ["serious", "critical"].includes(violation.impact ?? ""),
        ),
      ).toEqual([]);
    }
  });
}

test("Search aggregated results pass accessibility checks", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockCoreApi(page, { search: "partial" });
  await page.goto("/search");
  await page.getByLabel("搜索内容").fill("虚拟关键词");
  await page.getByRole("button", { name: "搜索" }).click();
  await expect(page.getByText("Virtual Search Album")).toBeVisible();
  const results = await new AxeBuilder({ page }).analyze();
  expect(
    results.violations.filter((violation) =>
      ["serious", "critical"].includes(violation.impact ?? ""),
    ),
  ).toEqual([]);
});
