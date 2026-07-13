import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import {
  discoverAlbumResourceId,
  discoverAlbumTitle,
  discoverPluginId,
  mockCoreApi,
} from "./fixtures/mock-api";

const viewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "tablet", width: 1024, height: 768 },
  { name: "mobile", width: 390, height: 844 },
] as const;
const routes = ["/discover", "/search", "/plugins", "/logs", "/system"];
const contentViewports = [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 390, height: 844 },
] as const;
const discoverLayouts = [
  "hero-carousel",
  "album-grid",
  "horizontal-list",
  "ranked-list",
  "category-grid",
] as const;

async function expectNoHorizontalOverflow(page: Page) {
  const dimensions = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth);
}

async function expectNoSeriousAccessibilityViolations(
  page: Page,
) {
  const results = await new AxeBuilder({ page }).analyze();
  expect(
    results.violations.filter((violation) =>
      ["serious", "critical"].includes(violation.impact ?? ""),
    ),
  ).toEqual([]);
}

async function expectVisibleSourceVersion(page: Page) {
  const pluginId = page
    .locator("[data-plugin-id]:visible", {
      hasText: discoverPluginId,
    })
    .first();
  await expect(pluginId).toBeVisible();
  await expect(pluginId.locator("..")).toContainText(
    "Virtual Content 1.0.0",
  );
}

async function openAlbumFromDiscover(page: Page) {
  const album = page.locator(
    `[data-resource-id="${discoverAlbumResourceId}"]`,
  );
  await expect(album).toBeVisible();
  await album.focus();
  await album.press("Enter");
  await expect(page).toHaveURL(/\/albums\/detail/);

  const url = new URL(page.url());
  expect(url.searchParams.get("pluginId")).toBe(discoverPluginId);
  expect(url.searchParams.get("resourceId")).toBe(
    discoverAlbumResourceId,
  );
}

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

      await expectNoSeriousAccessibilityViolations(page);
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
  await expectNoSeriousAccessibilityViolations(page);
});

for (const viewport of contentViewports) {
  test(`Discover five layouts pass ${viewport.name} accessibility checks`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await mockCoreApi(page, {
      discover: viewport.name === "mobile" ? "partial" : "results",
    });
    await page.goto("/discover");

    const layouts = page.locator("[data-discover-layout]");
    await expect(layouts).toHaveCount(discoverLayouts.length);
    for (const [index, layout] of discoverLayouts.entries()) {
      await expect(layouts.nth(index)).toHaveAttribute(
        "data-discover-layout",
        layout,
      );
    }
    await expect(page.getByText("Virtual Category")).toBeVisible();
    await expectVisibleSourceVersion(page);
    if (viewport.name === "mobile") {
      await expect(page.getByText("部分来源暂不可用")).toBeVisible();
      await expect(
        page.getByText("RESOURCE_ACCESS_DENIED"),
      ).toBeVisible();
    }

    await expectNoHorizontalOverflow(page);
    await expectNoSeriousAccessibilityViolations(page);
  });

  test(`Album detail passes ${viewport.name} accessibility and track pagination checks`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    await mockCoreApi(page, { discover: "results" });
    await page.goto("/discover");
    await openAlbumFromDiscover(page);

    await expect(
      page.getByRole("heading", {
        name: discoverAlbumTitle,
        exact: true,
      }),
    ).toBeVisible();
    await expect(page.getByText("Virtual Primary Creator")).toBeVisible();
    await expect(
      page.getByText(/Deterministic local album with/),
    ).toBeVisible();
    await expectVisibleSourceVersion(page);
    await expect(
      page.locator('[data-track-id="virtual-track-1"]'),
    ).toContainText("Virtual Track 1");

    const pagination = page.getByRole("navigation", {
      name: "内容分页",
    });
    await expect(pagination).toBeVisible();
    await pagination.getByRole("button", { name: "下一页" }).click();
    await expect(
      page.locator('[data-track-id="virtual-track-2"]'),
    ).toContainText("Virtual Track 2");
    await expect(
      page.locator('[data-track-id="virtual-track-1"]'),
    ).toHaveCount(0);
    await pagination.getByRole("button", { name: "上一页" }).click();
    await expect(
      page.locator('[data-track-id="virtual-track-1"]'),
    ).toContainText("Virtual Track 1");

    await expectNoHorizontalOverflow(page);
    await expectNoSeriousAccessibilityViolations(page);
  });
}

test("Album not-found state is safe and accessible", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockCoreApi(page);
  await page.goto(
    `/albums/detail?pluginId=${encodeURIComponent(discoverPluginId)}&resourceId=missing-album`,
  );

  await expect(page.getByRole("alert")).toContainText(
    "RESOURCE_NOT_FOUND",
  );
  await expect(page.locator("body")).not.toContainText(
    "raw-plugin-secret",
  );
  await expectNoHorizontalOverflow(page);
  await expectNoSeriousAccessibilityViolations(page);
});
