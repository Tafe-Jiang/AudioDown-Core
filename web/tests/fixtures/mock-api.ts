import type { Page, Route } from "@playwright/test";

export const longPluginId =
  "com.audiodown.virtual.content.with-an-intentionally-long-identifier-for-responsive-verification";
export const longCommitSha = "0123456789abcdef0123456789abcdef01234567";
export const longLogMessage =
  "虚拟插件在握手阶段返回了一段很长的结构化错误摘要，用于验证移动端日志内容能够自然换行且不会覆盖时间、级别或后续内容。";

const repositoryUrl =
  "https://github.com/example-owner/example-audiodown-plugin-repository";

export interface MockApiOptions {
  supervisorAvailable?: boolean;
  developmentMode?: boolean;
  repositoryInspectionError?: boolean;
}

function fulfillJson(route: Route, value: unknown, status = 200) {
  return route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(value),
  });
}

export async function mockCoreApi(
  page: Page,
  options: MockApiOptions = {},
) {
  const supervisorAvailable = options.supervisorAvailable ?? true;
  let plugins = [
    {
      pluginId: longPluginId,
      pluginType: "content",
      platformId: "virtual-content",
      name: "Virtual Content Plugin With A Long Responsive Name",
      version: "1.0.0",
      status: "installed",
      enabled: true,
      runMode: "on_demand",
      priority: 100,
      sourceUrl: repositoryUrl,
      commitSha: longCommitSha,
    },
  ];

  await page.route("**/api/v1/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const { pathname } = url;
    const method = request.method();

    if (pathname === "/api/v1/system") {
      return fulfillJson(route, {
        version: "1.0.0-alpha.1",
        supervisor: {
          available: supervisorAvailable,
          error: supervisorAvailable ? null : "Supervisor is unavailable",
        },
        pluginCount: plugins.length,
        developmentMode: options.developmentMode ?? true,
      });
    }

    if (pathname === "/api/v1/discover" || pathname === "/api/v1/search") {
      return fulfillJson(route, {
        reason: "NO_CONTENT_PLUGINS",
        title: "尚未安装内容插件",
        actionLabel: "添加 GitHub 插件仓库",
      });
    }

    if (pathname === "/api/v1/plugins" && method === "GET") {
      return fulfillJson(route, { items: plugins });
    }

    if (pathname === "/api/v1/logs") {
      return fulfillJson(route, {
        items: [
          {
            id: "018f0000-0000-7000-8000-000000000001",
            timestamp: "2026-07-12T08:30:00Z",
            level: "error",
            component: "plugin-runtime-with-a-long-component-name",
            message: longLogMessage,
            pluginId: longPluginId,
          },
        ],
      });
    }

    if (pathname === "/api/v1/plugin-repositories/inspect") {
      if (options.repositoryInspectionError) {
        return fulfillJson(route, { code: "INSPECTION_FAILED" }, 500);
      }
      return fulfillJson(route, {
        snapshotId: "018f0000-0000-7000-8000-000000000010",
        repository: {
          id: "example.plugins",
          name: "Example Plugin Repository With A Long Name",
          sourceUrl: repositoryUrl,
          commitSha: longCommitSha,
        },
        plugins: [
          {
            pluginId: "com.audiodown.virtual.risk-plugin",
            name: "Virtual Lifecycle Risk Plugin",
            version: "1.0.0",
            pluginType: "content",
            alreadyInstalled: false,
            requiresLifecycleScriptGrant: true,
            lifecycleScriptReason:
              "依赖安装阶段脚本，需要开发者模式下逐次明确授权。",
          },
        ],
      });
    }

    if (
      pathname.includes("/plugin-repositories/") &&
      pathname.endsWith("/install")
    ) {
      return fulfillJson(route, plugins[0]);
    }

    const pluginMatch = pathname.match(/^\/api\/v1\/plugins\/([^/]+)$/);
    if (pluginMatch && method === "PATCH") {
      const settings = request.postDataJSON() as {
        enabled: boolean;
        runMode: "on_demand" | "always";
        priority: number;
      };
      plugins = plugins.map((plugin) =>
        plugin.pluginId === decodeURIComponent(pluginMatch[1])
          ? { ...plugin, ...settings }
          : plugin,
      );
      return fulfillJson(route, plugins[0]);
    }
    if (pluginMatch && method === "DELETE") {
      plugins = plugins.filter(
        (plugin) => plugin.pluginId !== decodeURIComponent(pluginMatch[1]),
      );
      return route.fulfill({ status: 204 });
    }

    const runtimeMatch = pathname.match(
      /^\/api\/v1\/plugins\/([^/]+)\/(start|stop)$/,
    );
    if (runtimeMatch && method === "POST") {
      const status = runtimeMatch[2] === "start" ? "running" : "stopped";
      plugins = plugins.map((plugin) =>
        plugin.pluginId === decodeURIComponent(runtimeMatch[1])
          ? { ...plugin, status }
          : plugin,
      );
      return fulfillJson(route, {
        pluginId: decodeURIComponent(runtimeMatch[1]),
        status,
        logs: [],
      });
    }

    return fulfillJson(route, { code: "NOT_FOUND" }, 404);
  });
}
