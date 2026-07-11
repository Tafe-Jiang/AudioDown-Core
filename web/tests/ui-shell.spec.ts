import { expect, test, type Page } from "@playwright/test";

import {
  longCommitSha,
  longLogMessage,
  longPluginId,
  mockCoreApi,
} from "./fixtures/mock-api";

async function expectNoHorizontalOverflow(page: Page) {
  const dimensions = await page.evaluate(() => ({
    clientWidth: document.documentElement.clientWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.clientWidth);
}

test("desktop sidebar has stable expanded and collapsed dimensions", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockCoreApi(page);
  await page.goto("/plugins");

  await expect(page.locator('[data-slot="sidebar-gap"]')).toHaveCSS(
    "width",
    "232px",
  );
  await page.getByRole("button", { name: "折叠侧栏" }).click();
  await expect(page.locator('[data-slot="sidebar-gap"]')).toHaveCSS(
    "width",
    "64px",
  );
  await expectNoHorizontalOverflow(page);
});

test("mobile navigation traps focus and restores it to its trigger", async ({
  page,
}) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockCoreApi(page);
  await page.goto("/plugins");

  const trigger = page.getByRole("button", { name: "打开主导航" });
  await trigger.focus();
  await trigger.press("Enter");
  const sheet = page.locator('[data-mobile="true"]');
  await expect(sheet).toBeVisible();

  for (let index = 0; index < 8; index += 1) {
    await page.keyboard.press("Tab");
    expect(
      await page.evaluate(() =>
        Boolean(
          document.activeElement?.closest('[data-mobile="true"]'),
        ),
      ),
    ).toBe(true);
  }

  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();
  await expect(trigger).toBeFocused();
});

test("keyboard can inspect repositories and restore dialog focus", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1024, height: 768 });
  await mockCoreApi(page);
  await page.goto("/plugins");

  const addRepository = page.getByRole("button", { name: "添加仓库" }).first();
  await addRepository.focus();
  await addRepository.press("Enter");
  await expect(page.getByRole("dialog")).toBeVisible();

  await page
    .getByLabel("GitHub 公共仓库地址")
    .fill("https://github.com/example-owner/example-audiodown-plugin-repository");
  await page.getByRole("button", { name: "检查仓库" }).press("Enter");
  await expect(page.getByText(longCommitSha.slice(0, 7))).toBeVisible();
  await page.getByRole("button", { name: "返回" }).press("Enter");
  await page.getByRole("button", { name: "取消" }).press("Enter");
  await expect(page.getByRole("dialog")).toBeHidden();
  await expect(addRepository).toBeFocused();
});

test("keyboard can open settings and confirm named uninstall", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockCoreApi(page);
  await page.goto("/plugins");

  const more = page
    .getByRole("button", {
      name: "Virtual Content Plugin With A Long Responsive Name 更多操作",
    })
    .first();
  await more.focus();
  await more.press("Enter");
  await page.getByRole("menuitem", { name: "设置" }).press("Enter");
  await expect(page.getByRole("dialog", { name: /设置/ })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(more).toBeFocused();

  await more.press("Enter");
  await page.getByRole("menuitem", { name: "卸载" }).press("Enter");
  const confirmation = page.getByRole("alertdialog");
  await expect(confirmation).toContainText(
    "Virtual Content Plugin With A Long Responsive Name",
  );
  await page.getByRole("button", { name: "确认卸载" }).press("Enter");
  await expect(page.getByText("尚无已安装插件")).toBeVisible();
});

for (const viewport of [
  { name: "tablet", width: 1024, height: 768 },
  { name: "mobile", width: 390, height: 844 },
] as const) {
  test(`${viewport.name} long content does not overflow`, async ({ page }) => {
    await page.setViewportSize(viewport);
    await mockCoreApi(page);

    await page.goto("/plugins");
    if (viewport.width < 768) {
      await expect(
        page.locator(`[data-mobile-plugin-item="${longPluginId}"]`).getByText(
          longPluginId,
        ),
      ).toBeVisible();
    } else {
      await expect(page.getByText(longPluginId).first()).toBeVisible();
    }
    await expectNoHorizontalOverflow(page);

    await page.goto("/logs");
    if (viewport.width < 768) {
      await expect(
        page.locator("[data-mobile-log]").first().getByText(longLogMessage),
      ).toBeVisible();
    } else {
      await expect(page.getByText(longLogMessage).first()).toBeVisible();
    }
    await expectNoHorizontalOverflow(page);

    const mobileLog = page.locator("[data-mobile-log]").first();
    if (viewport.width < 768) {
      const box = await mobileLog.boundingBox();
      expect(box?.x ?? 0).toBeGreaterThanOrEqual(0);
      expect((box?.x ?? 0) + (box?.width ?? 0)).toBeLessThanOrEqual(
        viewport.width,
      );
    }
  });
}

test("reduced motion removes nonessential transition and animation duration", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await mockCoreApi(page);
  await page.goto("/plugins");

  const styles = await page.evaluate(() => {
    const sidebar = document.querySelector(
      '[data-slot="sidebar-container"]',
    ) as HTMLElement;
    const badge = document.querySelector('[data-slot="badge"]') as HTMLElement;
    return {
      transitionDuration: getComputedStyle(sidebar).transitionDuration,
      animationDuration: getComputedStyle(badge).animationDuration,
    };
  });

  expect(["0.01ms", "1e-05s"]).toContain(styles.transitionDuration);
  expect(["0.01ms", "1e-05s"]).toContain(styles.animationDuration);
});
