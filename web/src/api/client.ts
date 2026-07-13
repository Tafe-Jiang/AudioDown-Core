export interface EmptyStateResponse {
  reason: "NO_CONTENT_PLUGINS";
  title: string;
  actionLabel: string;
}

export interface ContentEmptyState extends EmptyStateResponse {}

export interface ContentSource {
  platformId: string;
  pluginId: string;
  pluginName: string;
  pluginVersion: string;
}

export type ContentResourceType = "album" | "track" | "category";

export interface ContentItem {
  resourceType: ContentResourceType;
  resourceId: string;
  canonicalId?: string;
  title: string;
  subtitle?: string;
  description?: string;
}

export interface SourcedContentItem {
  item: ContentItem;
  source: ContentSource;
}

export interface ContentFailure {
  code: string;
  summary: string;
  source: ContentSource;
}

export type DiscoverLayout =
  | "hero-carousel"
  | "album-grid"
  | "horizontal-list"
  | "ranked-list"
  | "category-grid";

export interface DiscoverSection {
  id: string;
  title: string;
  layout: DiscoverLayout;
  items: ContentItem[];
}

export interface SourcedDiscoverSection {
  section: DiscoverSection;
  source: ContentSource;
}

export interface CategoryItem {
  resourceId: string;
  canonicalId?: string;
  title: string;
  description?: string;
}

export interface SourcedCategoryItem {
  item: CategoryItem;
  source: ContentSource;
}

export interface ContentEnvelope {
  items: SourcedContentItem[];
  sections: SourcedDiscoverSection[];
  nextCursor: string | null;
  failures: ContentFailure[];
  emptyState: ContentEmptyState | null;
}

export interface CategoriesResponse {
  items: SourcedCategoryItem[];
  failures: ContentFailure[];
  emptyState: ContentEmptyState | null;
}

export interface ContentQueryOptions {
  platformId?: string;
  pluginId?: string;
  cursor?: string;
  limit?: number;
}

export interface SearchOptions extends ContentQueryOptions {
  query: string;
}

export interface DiscoverOptions extends ContentQueryOptions {}

export interface AlbumDetail {
  resourceId: string;
  canonicalId?: string;
  title: string;
  creator?: string;
  description?: string;
  trackCount?: number;
}

export interface AlbumResponse {
  album: AlbumDetail;
  source: ContentSource;
}

export interface TrackItem {
  resourceId: string;
  canonicalId?: string;
  title: string;
  subtitle?: string;
  sequence?: number;
  durationSeconds?: number;
}

export interface TracksResponse {
  items: TrackItem[];
  source: ContentSource;
  nextCursor: string | null;
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
  capabilities: string[];
  searchEnabled: boolean | null;
  discoverEnabled: boolean | null;
  isDefaultContentPlugin: boolean;
}

export interface PluginListResponse {
  items: PluginItem[];
}

export interface PluginSettings {
  enabled: boolean;
  runMode: PluginRunMode;
  priority: number;
  searchEnabled?: boolean;
  discoverEnabled?: boolean;
  defaultContentPluginId?: string;
}

export interface PluginRuntimeSettings {
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
  credentials: CredentialDeclarations;
}

export interface CredentialScopeDeclaration {
  scope: string;
  targetOrigins: string[];
}

export interface CredentialDeclarations {
  providedScopes: CredentialScopeDeclaration[];
  requiredScopes: CredentialScopeDeclaration[];
  optionalScopes: CredentialScopeDeclaration[];
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
  method?: "GET" | "POST" | "PATCH" | "PUT" | "DELETE";
  body?: unknown;
  headers?: Record<string, string>;
}

interface ApiErrorBody {
  code?: string;
  message?: string;
}

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
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
    const error = (await response
      .json()
      .catch(() => null)) as ApiErrorBody | null;
    throw new ApiError(
      response.status,
      error?.code ?? "CORE_API_ERROR",
      error?.message ?? `Core API request failed with status ${response.status}`,
    );
  }
  return response;
}

function queryString(values: Record<string, string | number | undefined>) {
  const query = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined && value !== "") {
      query.set(key, String(value));
    }
  }
  const serialized = query.toString();
  return serialized.length > 0 ? `?${serialized}` : "";
}

function legacyEmptyState(envelope: ContentEnvelope): EmptyStateResponse {
  return (
    envelope.emptyState ?? {
      reason: "NO_CONTENT_PLUGINS",
      title: "暂无内容插件",
      actionLabel: "管理插件",
    }
  );
}

function isLegacyEmptyState(
  response: ContentEnvelope | EmptyStateResponse,
): response is EmptyStateResponse {
  return (
    "reason" in response &&
    response.reason === "NO_CONTENT_PLUGINS" &&
    typeof response.title === "string" &&
    typeof response.actionLabel === "string"
  );
}

function emptyEnvelope(emptyState: EmptyStateResponse): ContentEnvelope {
  return {
    items: [],
    sections: [],
    nextCursor: null,
    failures: [],
    emptyState,
  };
}

function search(query: string): Promise<EmptyStateResponse>;
function search(options: SearchOptions): Promise<ContentEnvelope>;
async function search(
  input: string | SearchOptions,
): Promise<EmptyStateResponse | ContentEnvelope> {
  const options = typeof input === "string" ? { query: input } : input;
  const response = await requestJson<ContentEnvelope | EmptyStateResponse>(
    `/api/v1/search${queryString({
      q: options.query,
      platformId: options.platformId,
      pluginId: options.pluginId,
      cursor: options.cursor,
      limit: options.limit,
    })}`,
  );
  if (typeof input === "string") {
    return isLegacyEmptyState(response)
      ? response
      : legacyEmptyState(response);
  }
  return isLegacyEmptyState(response) ? emptyEnvelope(response) : response;
}

function discover(): Promise<EmptyStateResponse>;
function discover(options: DiscoverOptions): Promise<ContentEnvelope>;
async function discover(
  options?: DiscoverOptions,
): Promise<EmptyStateResponse | ContentEnvelope> {
  const response = await requestJson<ContentEnvelope | EmptyStateResponse>(
    `/api/v1/discover${queryString({
      platformId: options?.platformId,
      pluginId: options?.pluginId,
      cursor: options?.cursor,
      limit: options?.limit,
    })}`,
  );
  if (options === undefined) {
    return isLegacyEmptyState(response)
      ? response
      : legacyEmptyState(response);
  }
  return isLegacyEmptyState(response) ? emptyEnvelope(response) : response;
}

export const api = {
  discover,
  search,
  categories: (options: Pick<ContentQueryOptions, "platformId" | "pluginId"> = {}) =>
    requestJson<CategoriesResponse>(
      `/api/v1/categories${queryString(options)}`,
    ),
  album: (pluginId: string, resourceId: string) =>
    requestJson<AlbumResponse>("/api/v1/albums/get", {
      method: "POST",
      body: { pluginId, resourceId },
    }),
  tracks: (
    pluginId: string,
    albumResourceId: string,
    cursor?: string,
    limit = 20,
  ) =>
    requestJson<TracksResponse>("/api/v1/tracks/list", {
      method: "POST",
      body: { pluginId, albumResourceId, cursor, limit },
    }),
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
  updatePlugin: (pluginId: string, settings: PluginRuntimeSettings) =>
    requestJson<PluginItem>(
      `/api/v1/plugins/${encodeURIComponent(pluginId)}`,
      {
        method: "PATCH",
        body: settings,
      },
    ),
  updateContentSettings: (
    pluginId: string,
    searchEnabled: boolean,
    discoverEnabled: boolean,
  ) =>
    requestJson<{
      pluginId: string;
      searchEnabled: boolean;
      discoverEnabled: boolean;
    }>(
      `/api/v1/plugins/${encodeURIComponent(pluginId)}/content-settings`,
      {
        method: "PATCH",
        body: { searchEnabled, discoverEnabled },
      },
    ),
  setDefaultContentPlugin: (platformId: string, pluginId: string) =>
    requestJson<{ platformId: string; pluginId: string }>(
      `/api/v1/platforms/${encodeURIComponent(platformId)}/default-content-plugin`,
      {
        method: "PUT",
        body: { pluginId },
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
