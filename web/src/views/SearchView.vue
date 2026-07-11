<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { LoaderCircle, RotateCcw, Search } from "@lucide/vue";
import { useRouter } from "vue-router";

import {
  ApiError,
  api,
  type ContentEnvelope,
  type PluginItem,
} from "../api/client";
import EmptyState from "../components/common/EmptyState.vue";
import PageHeader from "../components/common/PageHeader.vue";
import ContentFailureAlert from "../components/content/ContentFailureAlert.vue";
import ContentGrid from "../components/content/ContentGrid.vue";
import ContentPagination from "../components/content/ContentPagination.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../components/ui/alert";
import { Button } from "../components/ui/button";
import { Field, FieldLabel } from "../components/ui/field";
import { Input } from "../components/ui/input";

const router = useRouter();
const query = ref("");
const platformId = ref("");
const pluginId = ref("");
const plugins = ref<PluginItem[]>([]);
const state = ref<ContentEnvelope | null>(null);
const validationError = ref("");
const requestError = ref("");
const loading = ref(false);
const cursorHistory = ref<Array<string | undefined>>([]);
const currentCursor = ref<string | undefined>();

interface ActiveSearchRequest {
  query: string;
  platformId?: string;
  pluginId?: string;
}

interface SearchAttempt {
  request: ActiveSearchRequest;
  cursor: string | undefined;
  history: Array<string | undefined>;
  activate: boolean;
}

const activeRequest = ref<ActiveSearchRequest | null>(null);
const lastAttempt = ref<SearchAttempt | null>(null);

const contentPlugins = computed(() =>
  plugins.value.filter(
    (plugin) =>
      plugin.pluginType === "content" &&
      plugin.enabled &&
      plugin.searchEnabled !== false &&
      plugin.capabilities.includes("content.search"),
  ),
);
const platforms = computed(() => {
  const values = new Map<string, string>();
  for (const plugin of contentPlugins.value) {
    if (!values.has(plugin.platformId)) {
      values.set(plugin.platformId, plugin.platformId);
    }
  }
  return [...values.entries()].map(([id, name]) => ({ id, name }));
});
const pluginOptions = computed(() =>
  contentPlugins.value.filter(
    (plugin) =>
      platformId.value.length === 0 ||
      plugin.platformId === platformId.value,
  ),
);
const hasSearched = computed(() => state.value !== null);
const noResults = computed(
  () =>
    hasSearched.value &&
    state.value?.items.length === 0 &&
    !state.value.emptyState &&
    state.value.failures.length === 0 &&
    cursorHistory.value.length === 0 &&
    !state.value.nextCursor,
);

watch(platformId, () => {
  if (
    pluginId.value &&
    !pluginOptions.value.some(
      (plugin) => plugin.pluginId === pluginId.value,
    )
  ) {
    pluginId.value = "";
  }
});

async function loadPlugins() {
  try {
    const response = await api.plugins();
    plugins.value = Array.isArray(response.items) ? response.items : [];
  } catch {
    plugins.value = [];
  }
}

async function executeSearch(
  request: ActiveSearchRequest,
  cursor: string | undefined,
  history: Array<string | undefined>,
  activate: boolean,
) {
  lastAttempt.value = {
    request: { ...request },
    cursor,
    history: [...history],
    activate,
  };
  loading.value = true;
  requestError.value = "";
  try {
    state.value = await api.search({
      ...request,
      cursor,
    });
    if (activate) {
      activeRequest.value = { ...request };
    }
    currentCursor.value = cursor;
    cursorHistory.value = history;
  } catch (error) {
    requestError.value =
      error instanceof ApiError
        ? `${error.code}: ${error.message}`
        : "无法读取搜索结果";
  } finally {
    loading.value = false;
  }
}

async function submitSearch() {
  validationError.value = "";
  const normalized = query.value.trim();
  if (!normalized) {
    validationError.value = "请输入搜索关键词";
    return;
  }
  query.value = normalized;
  await executeSearch(
    {
      query: normalized,
      platformId: platformId.value || undefined,
      pluginId: pluginId.value || undefined,
    },
    undefined,
    [],
    true,
  );
}

async function nextPage(cursor: string) {
  if (!activeRequest.value) {
    return;
  }
  await executeSearch(
    activeRequest.value,
    cursor,
    [...cursorHistory.value, currentCursor.value],
    false,
  );
}

async function previousPage() {
  if (cursorHistory.value.length === 0) {
    return;
  }
  const history = [...cursorHistory.value];
  const cursor = history.pop();
  if (activeRequest.value) {
    await executeSearch(
      activeRequest.value,
      cursor,
      history,
      false,
    );
  }
}

async function retrySearch() {
  if (!lastAttempt.value) {
    return;
  }
  await executeSearch(
    lastAttempt.value.request,
    lastAttempt.value.cursor,
    lastAttempt.value.history,
    lastAttempt.value.activate,
  );
}

onMounted(loadPlugins);
</script>

<template>
  <section class="grid min-w-0 gap-5">
    <PageHeader
      title="搜索"
      description="搜索已安装内容插件提供的内容。"
    />

    <form
      class="grid min-w-0 gap-3 lg:grid-cols-[minmax(16rem,1fr)_12rem_minmax(14rem,18rem)_auto]"
      @submit.prevent="submitSearch"
    >
      <Field class="min-w-0">
        <FieldLabel for="search-query">搜索内容</FieldLabel>
        <Input
          id="search-query"
          v-model="query"
          class="h-9"
          type="search"
          placeholder="输入关键词"
          :aria-invalid="validationError ? 'true' : undefined"
          @input="validationError = ''"
        />
      </Field>

      <Field class="min-w-0">
        <FieldLabel for="search-platform">平台</FieldLabel>
        <select
          id="search-platform"
          v-model="platformId"
          name="platform"
          class="h-9 w-full min-w-0 rounded-[5px] border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
          :disabled="loading"
        >
          <option value="">全部平台</option>
          <option
            v-for="platform in platforms"
            :key="platform.id"
            :value="platform.id"
          >
            {{ platform.name }}
          </option>
        </select>
      </Field>

      <Field class="min-w-0">
        <FieldLabel for="search-plugin">插件</FieldLabel>
        <select
          id="search-plugin"
          v-model="pluginId"
          name="plugin"
          class="h-9 w-full min-w-0 rounded-[5px] border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
          :disabled="loading"
        >
          <option value="">全部插件</option>
          <option
            v-for="plugin in pluginOptions"
            :key="plugin.pluginId"
            :value="plugin.pluginId"
          >
            {{ plugin.name }} {{ plugin.version }}
          </option>
        </select>
      </Field>

      <div class="flex items-end">
        <Button
          class="h-9 w-full lg:w-auto"
          type="submit"
          :disabled="loading"
        >
          <LoaderCircle
            v-if="loading"
            class="animate-spin"
            aria-hidden="true"
          />
          <Search v-else aria-hidden="true" />
          {{ loading ? "查询中" : "搜索" }}
        </Button>
      </div>
    </form>

    <Alert v-if="validationError" variant="destructive">
      <AlertTitle>无法搜索</AlertTitle>
      <AlertDescription>{{ validationError }}</AlertDescription>
    </Alert>

    <Alert v-if="requestError" variant="destructive">
      <AlertTitle>搜索失败</AlertTitle>
      <AlertDescription>{{ requestError }}</AlertDescription>
      <Button
        class="mt-2 w-fit"
        type="button"
        variant="outline"
        size="sm"
        aria-label="重试"
        :disabled="loading"
        @click="retrySearch"
      >
        <RotateCcw aria-hidden="true" />
        重试
      </Button>
    </Alert>

    <template v-if="state">
      <ContentFailureAlert :failures="state.failures" />

      <EmptyState
        v-if="state.emptyState"
        class="min-h-[360px] border-0 bg-transparent"
        :title="state.emptyState.title"
        description="安装内容插件后即可执行聚合搜索。"
        :primary-label="state.emptyState.actionLabel"
        @primary="router.push('/plugins')"
      >
        <template #icon>
          <Search aria-hidden="true" />
        </template>
      </EmptyState>

      <EmptyState
        v-else-if="noResults"
        class="min-h-64 border-0 bg-transparent"
        title="没有匹配结果"
        description="调整关键词或筛选条件后重试。"
      >
        <template #icon>
          <Search aria-hidden="true" />
        </template>
      </EmptyState>

      <template v-else>
        <div class="flex items-center justify-between gap-3">
          <h2 class="text-sm font-semibold">搜索结果</h2>
          <span class="text-xs text-muted-foreground">
            {{ state.items.length }} 项
          </span>
        </div>
        <ContentGrid :items="state.items" :loading="loading" />
        <ContentPagination
          :has-previous="cursorHistory.length > 0"
          :next-cursor="state.nextCursor"
          :busy="loading"
          @previous="previousPage"
          @next="nextPage"
        />
      </template>
    </template>
  </section>
</template>
