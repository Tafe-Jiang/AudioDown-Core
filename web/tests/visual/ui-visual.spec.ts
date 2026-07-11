import { expect, test, type Page } from "@playwright/test";

import {
  longCommitSha,
  longLogMessage,
  mockCoreApi,
} from "../fixtures/mock-api";

const desktop = { width: 1440, height: 900 };
const mobile = { width: 390, height: 844 };
const legacySelector = [
  ".brand-mark",
  ".nav-marker",
  ".empty-signal",
  ".summary-strip",
  ".data-table",
  ".system-list",
  ".primary-action",
].join(",");

async function expectVisual(page: Page, name: string) {
  await expect(page.locator(legacySelector)).toHaveCount(0);
  const layout = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
    gradients: [...document.querySelectorAll("*")].filter((element) =>
      getComputedStyle(element).backgroundImage.includes("gradient"),
    ).length,
  }));
  expect(layout.scrollWidth).toBeLessThanOrEqual(layout.clientWidth);
  expect(layout.gradients).toBe(0);
  await expect(page).toHaveScreenshot(name, { fullPage: true });
}

async function openRepositoryPreview(
  page: Page,
  repositoryRisk: boolean,
) {
  await mockCoreApi(page, {
    plugins: "empty",
    repositoryRisk,
    developmentMode: true,
  });
  await page.goto("/plugins");
  await page.getByRole("button", { name: "添加仓库" }).first().click();
  await page
    .getByLabel("GitHub 公共仓库地址")
    .fill("https://github.com/example-owner/example-audiodown-plugin-repository");
  await page.getByRole("button", { name: "检查仓库" }).click();
  await expect(page.getByText(longCommitSha.slice(0, 7))).toBeVisible();
}

test("desktop shell expanded", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page);
  await page.goto("/discover");
  await expect(page.getByRole("heading", { name: "发现" })).toBeVisible();
  await expectVisual(page, "desktop-shell-expanded.png");
});

test("desktop shell collapsed", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page);
  await page.goto("/discover");
  await page.getByRole("button", { name: "折叠侧栏" }).click();
  await expect(page.locator('[data-slot="sidebar-gap"]')).toHaveCSS(
    "width",
    "64px",
  );
  await expectVisual(page, "desktop-shell-collapsed.png");
});

test("mobile navigation open", async ({ page }) => {
  await page.setViewportSize(mobile);
  await mockCoreApi(page);
  await page.goto("/discover");
  await page.getByRole("button", { name: "打开主导航" }).click();
  const navigation = page.locator('[data-mobile="true"]');
  await expect(navigation).toBeVisible();

  const controls = navigation.locator("a:visible, button:visible");
  for (let index = 0; index < (await controls.count()); index += 1) {
    const box = await controls.nth(index).boundingBox();
    expect(box?.height ?? 0).toBeGreaterThanOrEqual(40);
  }
  await expectVisual(page, "mobile-navigation-open.png");
});

test("Discover empty", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, { plugins: "empty" });
  await page.goto("/discover");
  await expect(page.getByText("尚未安装内容插件")).toBeVisible();
  await expectVisual(page, "discover-empty.png");
});

test("Search empty with query", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, { plugins: "empty" });
  await page.goto("/search");
  await page.getByLabel("搜索内容").fill("虚拟关键词");
  await page.getByRole("button", { name: "搜索" }).click();
  await expect(page.getByText("尚未安装内容插件")).toBeVisible();
  await expectVisual(page, "search-empty-with-query.png");
});

test("Search aggregated results", async ({ page }) => {
  await page.setViewportSize(mobile);
  await mockCoreApi(page, { search: "partial" });
  await page.goto("/search");
  await page.getByLabel("搜索内容").fill("虚拟关键词");
  await page.getByRole("button", { name: "搜索" }).click();
  await expect(page.getByText("Virtual Search Album")).toBeVisible();
  await expect(page.getByText("部分来源暂不可用")).toBeVisible();
  for (const locator of [
    page.locator("form"),
    page.locator("[data-plugin-id]").first(),
    page.getByRole("navigation", { name: "内容分页" }),
  ]) {
    const box = await locator.boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(mobile.width);
  }
  await expectVisual(page, "search-aggregated-results.png");
});

test("Search aggregated results desktop", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, { search: "results" });
  await page.goto("/search");
  await page.getByLabel("搜索内容").fill("虚拟关键词");
  await page.getByRole("button", { name: "搜索" }).click();
  await expect(page.getByText("Virtual Search Album")).toBeVisible();
  await expectVisual(page, "search-aggregated-results-desktop.png");
});

test("Plugins empty", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, { plugins: "empty" });
  await page.goto("/plugins");
  await expect(page.getByText("尚无已安装插件")).toBeVisible();
  await expectVisual(page, "plugins-empty.png");
});

test("repository preview normal", async ({ page }) => {
  await page.setViewportSize(desktop);
  await openRepositoryPreview(page, false);
  await expect(page.getByText("Virtual Content Plugin")).toBeVisible();
  await expect(page.getByText("安装脚本风险授权")).toHaveCount(0);
  await expectVisual(page, "repository-preview-normal.png");
});

test("repository preview lifecycle risk", async ({ page }) => {
  await page.setViewportSize(desktop);
  await openRepositoryPreview(page, true);
  await expect(page.getByText("安装脚本风险授权")).toBeVisible();
  await expectVisual(page, "repository-preview-lifecycle-risk.png");
});

test("installed plugin table", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page);
  await page.goto("/plugins");
  await expect(page.locator("[data-desktop-plugin-table]")).toBeVisible();
  await expectVisual(page, "installed-plugin-table.png");
});

test("plugin settings sheet", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page);
  await page.goto("/plugins");
  await page.getByRole("button", { name: /更多操作/ }).first().click();
  await page.getByRole("menuitem", { name: "设置" }).click();
  await expect(page.getByRole("dialog", { name: /设置/ })).toBeVisible();
  await expectVisual(page, "plugin-settings-sheet.png");
});

test("uninstall confirmation", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page);
  await page.goto("/plugins");
  await page.getByRole("button", { name: /更多操作/ }).first().click();
  await page.getByRole("menuitem", { name: "卸载" }).click();
  await expect(page.getByRole("alertdialog")).toBeVisible();
  await expectVisual(page, "uninstall-confirmation.png");
});

test("Logs empty", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, { logs: "empty" });
  await page.goto("/logs");
  await expect(page.getByText("暂无结构化日志")).toBeVisible();
  await expectVisual(page, "logs-empty.png");
});

test("Logs populated with long message", async ({ page }) => {
  await page.setViewportSize(mobile);
  await mockCoreApi(page, { logs: "populated" });
  await page.goto("/logs");
  await expect(
    page.locator("[data-mobile-log]").getByText(longLogMessage),
  ).toBeVisible();
  await expectVisual(page, "logs-populated-long-message.png");
});

test("System healthy", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, {
    supervisorAvailable: true,
    developmentMode: false,
  });
  await page.goto("/system");
  await expect(page.getByText("未启用")).toBeVisible();
  await expectVisual(page, "system-healthy.png");
});

test("System Supervisor unavailable", async ({ page }) => {
  await page.setViewportSize(desktop);
  await mockCoreApi(page, {
    supervisorAvailable: false,
    developmentMode: false,
  });
  await page.goto("/system");
  await expect(page.getByText("Supervisor 当前不可用")).toBeVisible();
  await expectVisual(page, "system-supervisor-unavailable.png");
});
