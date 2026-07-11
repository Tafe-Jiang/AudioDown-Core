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
}

export interface PluginItem {
  pluginId: string;
  name: string;
  version: string;
  status: string;
  enabled: boolean;
}

export interface PluginListResponse {
  items: PluginItem[];
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

async function getJson<T>(path: string): Promise<T> {
  const response = await fetch(path, {
    headers: { Accept: "application/json" },
  });
  if (!response.ok) {
    throw new Error(`Core API request failed with status ${response.status}`);
  }
  return response.json() as Promise<T>;
}

export const api = {
  discover: () => getJson<EmptyStateResponse>("/api/v1/discover"),
  search: (query: string) =>
    getJson<EmptyStateResponse>(
      `/api/v1/search?q=${encodeURIComponent(query)}`,
    ),
  plugins: () => getJson<PluginListResponse>("/api/v1/plugins"),
  logs: () => getJson<LogListResponse>("/api/v1/logs"),
  system: () => getJson<SystemResponse>("/api/v1/system"),
};
