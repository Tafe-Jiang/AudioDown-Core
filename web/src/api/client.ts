export interface EmptyStateResponse {
  reason: "NO_CONTENT_PLUGINS";
  title: string;
  actionLabel: string;
}

export interface SupervisorStatus {
  available: boolean;
  error: string | null;
}

export interface SystemResponse {
  version: string;
  supervisor: SupervisorStatus;
  pluginCount: number;
  developmentMode: boolean;
}

export type PluginRunMode = "on_demand" | "always";

export interface PluginItem {
  pluginId: string;
  pluginType: "content" | "credential";
  platformId: string;
  name: string;
  version: string;
  status: string;
  enabled: boolean;
  runMode: PluginRunMode;
  priority: number;
  sourceUrl: string;
  commitSha: string;
}

export interface PluginListResponse {
  items: PluginItem[];
}

export interface PluginSettings {
  enabled: boolean;
  runMode: PluginRunMode;
  priority: number;
}

export interface PluginRuntimeState {
  pluginId: string;
  status: string;
  containerId?: string;
  logs?: Array<{
    level: string;
    message: string;
    context: Record<string, unknown>;
  }>;
}

export interface StructuredLog {
  id: string;
  timestamp: string;
  level: string;
  component: string;
  message: string;
  pluginId: string | null;
}

export interface LogListResponse {
  items: StructuredLog[];
}

export interface RepositoryPluginPreview {
  pluginId: string;
  name: string;
  version: string;
  pluginType: "content" | "credential";
  alreadyInstalled: boolean;
  requiresLifecycleScriptGrant: boolean;
  lifecycleScriptReason: string | null;
}

export interface RepositoryPreview {
  snapshotId: string;
  repository: {
    id: string;
    name: string;
    sourceUrl: string;
    commitSha: string;
  };
  plugins: RepositoryPluginPreview[];
}

interface JsonRequestOptions {
  method?: "GET" | "POST" | "PATCH" | "DELETE";
  body?: unknown;
  headers?: Record<string, string>;
}

async function requestJson<T>(
  path: string,
  options: JsonRequestOptions = {},
): Promise<T> {
  const response = await request(path, options);
  return response.json() as Promise<T>;
}

async function request(
  path: string,
  options: JsonRequestOptions = {},
): Promise<Response> {
  const headers: Record<string, string> = {
    Accept: "application/json",
    ...options.headers,
  };
  const body =
    options.body === undefined ? undefined : JSON.stringify(options.body);
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
  }

  const response = await fetch(path, {
    method: options.method,
    headers,
    body,
  });
  if (!response.ok) {
    throw new Error(`Core API request failed with status ${response.status}`);
  }
  return response;
}

export const api = {
  discover: () => requestJson<EmptyStateResponse>("/api/v1/discover"),
  search: (query: string) =>
    requestJson<EmptyStateResponse>(
      `/api/v1/search?q=${encodeURIComponent(query)}`,
    ),
  plugins: () => requestJson<PluginListResponse>("/api/v1/plugins"),
  logs: () => requestJson<LogListResponse>("/api/v1/logs"),
  system: () => requestJson<SystemResponse>("/api/v1/system"),
  inspectRepository: (url: string) =>
    requestJson<RepositoryPreview>("/api/v1/plugin-repositories/inspect", {
      method: "POST",
      body: { url },
    }),
  installPlugin: (
    snapshotId: string,
    pluginId: string,
    allowLifecycleScripts: boolean,
    developerToken?: string,
  ) =>
    requestJson<PluginItem>(
      `/api/v1/plugin-repositories/${encodeURIComponent(snapshotId)}/plugins/${encodeURIComponent(pluginId)}/install`,
      {
        method: "POST",
        body: { allowLifecycleScripts },
        headers:
          developerToken && developerToken.length > 0
            ? { "x-audiodown-dev-token": developerToken }
            : undefined,
      },
    ),
  updatePlugin: (pluginId: string, settings: PluginSettings) =>
    requestJson<PluginItem>(
      `/api/v1/plugins/${encodeURIComponent(pluginId)}`,
      {
        method: "PATCH",
        body: settings,
      },
    ),
  startPlugin: (pluginId: string) =>
    requestJson<PluginRuntimeState>(
      `/api/v1/plugins/${encodeURIComponent(pluginId)}/start`,
      { method: "POST" },
    ),
  stopPlugin: (pluginId: string) =>
    requestJson<PluginRuntimeState>(
      `/api/v1/plugins/${encodeURIComponent(pluginId)}/stop`,
      { method: "POST" },
    ),
  uninstallPlugin: async (pluginId: string) => {
    await request(`/api/v1/plugins/${encodeURIComponent(pluginId)}`, {
      method: "DELETE",
    });
  },
};
