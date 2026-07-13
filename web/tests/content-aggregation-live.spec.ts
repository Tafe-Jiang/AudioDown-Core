import {
  expect,
  test,
  type APIRequestContext,
  type APIResponse,
  type Page,
} from "@playwright/test";

const primaryPluginId = "com.audiodown.virtual.content";
const backupPluginId = "com.audiodown.virtual.content-backup";
const catalogPluginId = "com.audiodown.catalog.content";
const contentPluginIds = [
  primaryPluginId,
  backupPluginId,
  catalogPluginId,
] as const;
const contentMethods = [
  "content.search",
  "content.discover",
  "content.categories",
  "content.album.get",
  "content.tracks.list",
] as const;
const primaryAlbumId = "virtual-album-1";

const liveBaseURL = process.env.AUDIODOWN_LIVE_BASE_URL?.trim();

test.use({
  baseURL: liveBaseURL ?? "http://127.0.0.1:4173",
  trace: "off",
  screenshot: "off",
  video: "off",
});

test.skip(
  !liveBaseURL,
  "AUDIODOWN_LIVE_BASE_URL is required for this live Core test",
);

interface PluginItem {
  pluginId: string;
  pluginType: string;
  platformId: string;
  enabled: boolean;
  searchEnabled: boolean | null;
  discoverEnabled: boolean | null;
  isDefaultContentPlugin: boolean;
  capabilities: string[];
}

interface PluginListResponse {
  items: PluginItem[];
}

interface StructuredLog {
  component: string;
  message: string;
  pluginId?: string | null;
  platformId?: string | null;
  context?: {
    method?: string;
  };
}

interface LogListResponse {
  items: StructuredLog[];
}

async function json<T>(response: APIResponse): Promise<T> {
  expect(response.ok(), await response.text()).toBe(true);
  return response.json() as Promise<T>;
}

async function requireInstalledContentPlugins(
  request: APIRequestContext,
): Promise<PluginItem[]> {
  const response = await request.get("/api/v1/plugins");
  const plugins = (await json<PluginListResponse>(response)).items;

  for (const pluginId of contentPluginIds) {
    const plugin = plugins.find((candidate) => candidate.pluginId === pluginId);
    expect(
      plugin,
      `Live smoke must preinstall ${pluginId} before running this spec`,
    ).toBeDefined();
    expect(plugin).toMatchObject({
      pluginId,
      pluginType: "content",
      enabled: true,
    });
    expect(plugin?.capabilities).toEqual(
      expect.arrayContaining(contentMethods),
    );
  }

  return plugins;
}

async function setParticipation(
  request: APIRequestContext,
  pluginId: string,
  searchEnabled: boolean,
  discoverEnabled: boolean,
) {
  const response = await request.patch(
    `/api/v1/plugins/${encodeURIComponent(pluginId)}/content-settings`,
    {
      data: { searchEnabled, discoverEnabled },
    },
  );
  await json(response);
}

async function setDefault(request: APIRequestContext, pluginId: string) {
  const response = await request.put(
    "/api/v1/platforms/virtual/default-content-plugin",
    { data: { pluginId } },
  );
  await json(response);
}

async function restoreRoutingAndStopPlugins(request: APIRequestContext) {
  for (const pluginId of contentPluginIds) {
    try {
      await request.patch(
        `/api/v1/plugins/${encodeURIComponent(pluginId)}/content-settings`,
        {
          data: { searchEnabled: true, discoverEnabled: true },
        },
      );
    } catch {
      // Cleanup is best-effort because the live smoke may not be wired yet.
    }
  }

  try {
    await request.put("/api/v1/platforms/virtual/default-content-plugin", {
      data: { pluginId: primaryPluginId },
    });
  } catch {
    // Cleanup is best-effort because the live smoke may not be wired yet.
  }

  for (const pluginId of contentPluginIds) {
    try {
      await request.post(
        `/api/v1/plugins/${encodeURIComponent(pluginId)}/stop`,
      );
    } catch {
      // Stopping an absent or already stopped plugin must not mask the test.
    }
  }
}

async function submitSearch(page: Page, query: string) {
  await page.getByLabel("搜索内容").fill(query);
  const responsePromise = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return (
      response.request().method() === "GET" &&
      url.pathname === "/api/v1/search" &&
      url.searchParams.get("q") === query
    );
  });
  await page.getByRole("button", { name: "搜索", exact: true }).click();
  const response = await responsePromise;
  expect(response.ok()).toBe(true);
}

test.afterEach(async ({ request }) => {
  await restoreRoutingAndStopPlugins(request);
});

test("searches aggregated content across virtual platforms through the real UI", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  await requireInstalledContentPlugins(request);
  await setParticipation(request, primaryPluginId, true, true);
  await setParticipation(request, backupPluginId, true, true);
  await setParticipation(request, catalogPluginId, true, true);
  await setDefault(request, primaryPluginId);

  await page.goto("/search");
  await expect(
    page.getByRole("heading", { name: "搜索", exact: true }),
  ).toBeVisible();
  await submitSearch(page, "fixture");

  await expect(page.getByText(catalogPluginId, { exact: true })).toBeVisible();
  await expect(page.getByText("Catalog Album")).toBeVisible();
  await expect(page.getByText(primaryPluginId, { exact: true })).toHaveCount(0);
  await expect(page.getByText("Virtual Primary Album")).toHaveCount(0);

  const logs = await json<LogListResponse>(
    await request.get("/api/v1/logs?limit=200"),
  );
  const searchedPlugins = new Set(
    logs.items
      .filter(
        (log) =>
          log.component === "plugin-content" &&
          log.context?.method === "content.search",
      )
      .map((log) => log.pluginId)
      .filter((pluginId): pluginId is string => Boolean(pluginId)),
  );
  expect(searchedPlugins).toContain(primaryPluginId);
  expect(searchedPlugins).toContain(catalogPluginId);
});

test("keeps successful UI state beside safe partial failures", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  await requireInstalledContentPlugins(request);
  await setParticipation(request, primaryPluginId, true, true);
  await setParticipation(request, backupPluginId, true, true);
  await setParticipation(request, catalogPluginId, true, true);
  await setDefault(request, primaryPluginId);

  await page.goto("/search");
  await submitSearch(page, "__retryable__");

  const partialFailure = page.getByRole("alert").filter({
    hasText: "部分来源暂不可用",
  });
  await expect(partialFailure).toBeVisible();
  await expect(partialFailure).toContainText("RATE_LIMITED");
  await expect(partialFailure).toContainText(primaryPluginId);
  await expect(page.getByText(catalogPluginId, { exact: true })).toBeVisible();
  await expect(page.getByText("Catalog Album")).toBeVisible();
  await expect(page.locator("body")).not.toContainText("raw-plugin-secret");
});

test("opens five discover layouts and paginates a source-bound album", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  await requireInstalledContentPlugins(request);
  await setParticipation(request, primaryPluginId, true, true);

  await page.goto("/discover");
  await expect(
    page.getByRole("heading", { name: "发现", exact: true }),
  ).toBeVisible();

  await page.locator("#discover-platform").selectOption("virtual");
  await page.locator("#discover-plugin").selectOption(primaryPluginId);
  const discoverResponsePromise = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return (
      response.request().method() === "GET" &&
      url.pathname === "/api/v1/discover" &&
      url.searchParams.get("pluginId") === primaryPluginId
    );
  });
  const categoriesResponsePromise = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return (
      response.request().method() === "GET" &&
      url.pathname === "/api/v1/categories" &&
      url.searchParams.get("pluginId") === primaryPluginId
    );
  });
  await page.getByRole("button", { name: "应用筛选", exact: true }).click();
  expect((await discoverResponsePromise).ok()).toBe(true);
  expect((await categoriesResponsePromise).ok()).toBe(true);

  for (const layout of [
    "hero-carousel",
    "album-grid",
    "horizontal-list",
    "ranked-list",
    "category-grid",
  ]) {
    await expect(
      page.locator(`[data-discover-layout="${layout}"]`),
    ).toHaveCount(1);
  }

  const albumResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/v1/albums/get",
  );
  const firstTracksResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/v1/tracks/list",
  );
  await page
    .locator(
      `[data-discover-layout="hero-carousel"] [data-resource-id="${primaryAlbumId}"]`,
    )
    .click();

  const albumResponse = await albumResponsePromise;
  const albumRequest = albumResponse.request().postDataJSON();
  expect(albumResponse.ok()).toBe(true);
  expect(albumRequest).toMatchObject({
    pluginId: primaryPluginId,
    resourceId: primaryAlbumId,
  });

  const firstTracksResponse = await firstTracksResponsePromise;
  const firstTracksRequest = firstTracksResponse.request().postDataJSON();
  expect(firstTracksResponse.ok()).toBe(true);
  expect(firstTracksRequest).toMatchObject({
    pluginId: primaryPluginId,
    albumResourceId: primaryAlbumId,
  });

  const albumUrl = new URL(page.url());
  expect(albumUrl.pathname).toBe("/albums/detail");
  expect(albumUrl.searchParams.get("pluginId")).toBe(primaryPluginId);
  expect(albumUrl.searchParams.get("resourceId")).toBe(primaryAlbumId);
  await expect(
    page.getByRole("heading", {
      name: "Virtual Primary Album",
      exact: true,
    }),
  ).toBeVisible();
  await expect(page.getByText(primaryPluginId, { exact: true })).toBeVisible();
  await expect(
    page.locator('[data-track-id="virtual-primary-track-1"]'),
  ).toBeVisible();

  const nextTracksResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/v1/tracks/list",
  );
  await page.getByRole("button", { name: "下一页" }).click();
  const nextTracksResponse = await nextTracksResponsePromise;
  expect(nextTracksResponse.ok()).toBe(true);
  expect(nextTracksResponse.request().postDataJSON()).toMatchObject({
    pluginId: primaryPluginId,
    albumResourceId: primaryAlbumId,
    cursor: "tracks-page-2",
  });
  await expect(
    page.locator('[data-track-id="virtual-primary-track-2"]'),
  ).toBeVisible();

  const previousTracksResponsePromise = page.waitForResponse(
    (response) =>
      response.request().method() === "POST" &&
      new URL(response.url()).pathname === "/api/v1/tracks/list",
  );
  await page.getByRole("button", { name: "上一页" }).click();
  const previousTracksResponse = await previousTracksResponsePromise;
  const previousTracksRequest = previousTracksResponse.request().postDataJSON();
  expect(previousTracksResponse.ok()).toBe(true);
  expect(previousTracksRequest).toMatchObject({
    pluginId: primaryPluginId,
    albumResourceId: primaryAlbumId,
  });
  expect(previousTracksRequest.cursor).toBeUndefined();
  await expect(
    page.locator('[data-track-id="virtual-primary-track-1"]'),
  ).toBeVisible();
});

test("applies participation and default routing, falls back, and records safe method logs", async ({
  page,
  request,
}) => {
  test.setTimeout(120_000);
  await requireInstalledContentPlugins(request);

  await setParticipation(request, primaryPluginId, true, true);
  await setParticipation(request, backupPluginId, true, true);
  await setParticipation(request, catalogPluginId, false, true);
  await setDefault(request, primaryPluginId);

  const configured = await requireInstalledContentPlugins(request);
  expect(
    configured.find((plugin) => plugin.pluginId === primaryPluginId),
  ).toMatchObject({
    searchEnabled: true,
    discoverEnabled: true,
    isDefaultContentPlugin: true,
  });
  expect(
    configured.find((plugin) => plugin.pluginId === backupPluginId),
  ).toMatchObject({
    searchEnabled: true,
    discoverEnabled: true,
  });
  expect(
    configured.find((plugin) => plugin.pluginId === catalogPluginId),
  ).toMatchObject({
    searchEnabled: false,
    discoverEnabled: true,
  });

  await page.goto("/search");
  await submitSearch(page, "__retryable__");

  const fallbackFailure = page.getByRole("alert").filter({
    hasText: "部分来源暂不可用",
  });
  await expect(fallbackFailure).toContainText("RATE_LIMITED");
  await expect(fallbackFailure).toContainText(primaryPluginId);
  await expect(page.getByText(backupPluginId, { exact: true })).toBeVisible();
  await expect(page.getByText(catalogPluginId, { exact: true })).toHaveCount(0);

  for (const response of [
    await request.get(
      `/api/v1/discover?pluginId=${encodeURIComponent(primaryPluginId)}`,
    ),
    await request.get(
      `/api/v1/categories?pluginId=${encodeURIComponent(primaryPluginId)}`,
    ),
    await request.post("/api/v1/albums/get", {
      data: {
        pluginId: primaryPluginId,
        resourceId: primaryAlbumId,
      },
    }),
    await request.post("/api/v1/tracks/list", {
      data: {
        pluginId: primaryPluginId,
        albumResourceId: primaryAlbumId,
        limit: 20,
      },
    }),
  ]) {
    expect(response.ok(), await response.text()).toBe(true);
  }

  const logsResponse = await request.get("/api/v1/logs?limit=200");
  const logs = await json<LogListResponse>(logsResponse);
  const loggedMethods = new Set(
    logs.items
      .filter((log) => log.component === "content-api")
      .map((log) => log.context?.method)
      .filter((method): method is string => Boolean(method)),
  );

  for (const method of contentMethods) {
    expect(loggedMethods, `Missing structured log for ${method}`).toContain(
      method,
    );
  }
  expect(JSON.stringify(logs)).not.toContain("raw-plugin-secret");
});
