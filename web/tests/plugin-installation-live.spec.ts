import { expect, test, type Page } from "@playwright/test";

const repositoryUrl =
  "https://github.com/example-owner/example-repository";
const repositoryId = "example.plugins";
const commitSha = "0123456789abcdef0123456789abcdef01234567";
const contentPluginId = "com.audiodown.virtual.content";
const buildRiskPluginId = "com.audiodown.virtual.build-risk";
const buildRiskPluginName = "Virtual Build Risk";
const updatedPriority = 37;

const liveBaseURL = process.env.AUDIODOWN_LIVE_BASE_URL?.trim();
const liveDeveloperToken = process.env.AUDIODOWN_LIVE_DEV_TOKEN;
const liveConfigured = Boolean(liveBaseURL && liveDeveloperToken);

if (Boolean(liveBaseURL) !== Boolean(liveDeveloperToken)) {
  throw new Error(
    "AUDIODOWN_LIVE_BASE_URL and AUDIODOWN_LIVE_DEV_TOKEN must be set together",
  );
}

test.use({
  baseURL: liveBaseURL ?? "http://127.0.0.1:4173",
  trace: "off",
  screenshot: "off",
  video: "off",
});

test.skip(
  !liveConfigured,
  "Live Core URL and developer token are required for this smoke test",
);

interface RepositoryInspection {
  repository: {
    id: string;
    sourceUrl: string;
    commitSha: string;
  };
  plugins: Array<{
    pluginId: string;
    name: string;
    requiresLifecycleScriptGrant: boolean;
  }>;
}

async function openPluginSettings(page: Page) {
  await page
    .getByRole("button", {
      name: `${buildRiskPluginName} 更多操作`,
    })
    .click();
  await page.getByRole("menuitem", { name: "设置" }).click();

  const settings = page.getByRole("dialog", {
    name: `设置 ${buildRiskPluginName}`,
  });
  await expect(settings).toBeVisible();
  return settings;
}

async function selectRunMode(
  page: Page,
  label: "持续运行" | "按需运行",
) {
  const settings = page.getByRole("dialog", {
    name: `设置 ${buildRiskPluginName}`,
  });
  await settings.getByRole("combobox", { name: "运行模式" }).click();
  await page.getByRole("option", { name: label }).click();
}

test("installs and manages the build-risk plugin through the real Core UI", async ({
  page,
}) => {
  test.setTimeout(180_000);

  await page.goto("/plugins");
  await expect(
    page.getByRole("heading", { name: "插件", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("尚无已安装插件")).toBeVisible();

  await page.getByRole("button", { name: "添加仓库" }).first().click();
  const repositoryDialog = page.getByRole("dialog", {
    name: "添加插件仓库",
  });
  await expect(repositoryDialog).toBeVisible();
  await repositoryDialog
    .getByLabel("GitHub 公共仓库地址")
    .fill(repositoryUrl);

  const inspectionResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname ===
        "/api/v1/plugin-repositories/inspect",
  );
  await repositoryDialog
    .getByRole("button", { name: "检查仓库" })
    .click();
  const inspectionResponse = await inspectionResponsePromise;
  expect(inspectionResponse.ok()).toBe(true);

  const inspection =
    (await inspectionResponse.json()) as RepositoryInspection;
  expect(inspection.repository).toMatchObject({
    id: repositoryId,
    sourceUrl: repositoryUrl,
    commitSha,
  });
  expect(inspection.plugins).toHaveLength(2);
  expect(inspection.plugins).toEqual(
    expect.arrayContaining([
      expect.objectContaining({
        pluginId: contentPluginId,
        name: "Virtual Content",
        requiresLifecycleScriptGrant: false,
      }),
      expect.objectContaining({
        pluginId: buildRiskPluginId,
        name: buildRiskPluginName,
        requiresLifecycleScriptGrant: true,
      }),
    ]),
  );

  await expect(
    repositoryDialog.getByText(repositoryUrl, { exact: true }),
  ).toBeVisible();
  await expect(
    repositoryDialog.getByText(commitSha.slice(0, 7), { exact: true }),
  ).toBeVisible();
  await expect(
    repositoryDialog.getByRole("button", { name: /Virtual Content/ }),
  ).toBeVisible();
  const buildRiskPlugin = repositoryDialog.getByRole("button", {
    name: /Virtual Build Risk/,
  });
  await expect(buildRiskPlugin).toBeVisible();
  await buildRiskPlugin.click();

  await expect(
    repositoryDialog.getByText("安装脚本风险授权"),
  ).toBeVisible();
  const riskApproval = repositoryDialog.getByRole("checkbox", {
    name: "我明确允许本次 Commit 执行 npm 安装脚本",
  });
  await expect(riskApproval).not.toBeChecked();
  await riskApproval.click();
  await expect(riskApproval).toBeChecked();

  const developerTokenInput =
    repositoryDialog.getByLabel("开发者令牌");
  await expect(developerTokenInput).toHaveAttribute("type", "password");
  await expect(developerTokenInput).toHaveAttribute("autocomplete", "off");
  await developerTokenInput.fill(liveDeveloperToken!);

  const installResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname.endsWith(
        `/plugins/${buildRiskPluginId}/install`,
      ),
    { timeout: 120_000 },
  );
  await repositoryDialog
    .getByRole("button", { name: "安装插件" })
    .click();
  const installResponse = await installResponsePromise;
  expect(installResponse.ok()).toBe(true);
  await expect(repositoryDialog).toBeHidden();

  const pluginRow = page
    .getByRole("row")
    .filter({ hasText: buildRiskPluginId });
  await expect(pluginRow).toContainText(buildRiskPluginName);
  await expect(pluginRow).toContainText("按需运行");
  await expect(pluginRow).toContainText("100");

  const enabledSwitch = pluginRow.getByRole("switch", {
    name: `${buildRiskPluginName} 启用状态`,
  });
  await expect(enabledSwitch).toBeChecked();
  await enabledSwitch.click();
  await expect(enabledSwitch).not.toBeChecked();
  await expect(enabledSwitch).toBeEnabled();
  await enabledSwitch.click();
  await expect(enabledSwitch).toBeChecked();
  await expect(enabledSwitch).toBeEnabled();

  let settings = await openPluginSettings(page);
  await selectRunMode(page, "持续运行");
  await settings.getByLabel("优先级").fill(String(updatedPriority));
  await settings.getByRole("button", { name: "保存" }).click();
  await expect(settings).toBeHidden();
  await expect(pluginRow).toContainText("持续运行");
  await expect(pluginRow).toContainText(String(updatedPriority));
  await expect(
    pluginRow.getByRole("button", {
      name: `停止 ${buildRiskPluginName}`,
    }),
  ).toBeVisible();

  settings = await openPluginSettings(page);
  await selectRunMode(page, "按需运行");
  await settings.getByRole("button", { name: "保存" }).click();
  await expect(settings).toBeHidden();
  await expect(pluginRow).toContainText("按需运行");
  await expect(pluginRow).toContainText(String(updatedPriority));

  await pluginRow
    .getByRole("button", { name: `停止 ${buildRiskPluginName}` })
    .click();
  const startButton = pluginRow.getByRole("button", {
    name: `启动 ${buildRiskPluginName}`,
  });
  await expect(startButton).toBeVisible();
  await startButton.click();
  const stopButton = pluginRow.getByRole("button", {
    name: `停止 ${buildRiskPluginName}`,
  });
  await expect(stopButton).toBeVisible();
  await stopButton.click();
  await expect(startButton).toBeVisible();

  await pluginRow
    .getByRole("button", {
      name: `${buildRiskPluginName} 更多操作`,
    })
    .click();
  await page.getByRole("menuitem", { name: "卸载" }).click();
  const uninstallDialog = page.getByRole("alertdialog", {
    name: `卸载 ${buildRiskPluginName}？`,
  });
  await expect(uninstallDialog).toBeVisible();

  const uninstallResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "DELETE" &&
      new URL(response.url()).pathname.endsWith(
        `/plugins/${buildRiskPluginId}`,
      ),
  );
  await uninstallDialog
    .getByRole("button", { name: "确认卸载" })
    .click();
  const uninstallResponse = await uninstallResponsePromise;
  expect(uninstallResponse.ok()).toBe(true);

  await expect(page.getByText("尚无已安装插件")).toBeVisible();
  await expect(page.getByText(buildRiskPluginId, { exact: true })).toHaveCount(
    0,
  );
});
