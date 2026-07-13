<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import {
  ArrowUpRight,
  Compass,
  Disc3,
  LoaderCircle,
  RotateCcw,
  Tags,
} from "@lucide/vue";
import { RouterLink, useRouter } from "vue-router";

import {
  ApiError,
  api,
  type CategoriesResponse,
  type ContentEnvelope,
  type ContentFailure,
  type ContentItem,
  type ContentSource,
  type DiscoverLayout,
  type PluginItem,
} from "../api/client";
import EmptyState from "../components/common/EmptyState.vue";
import PageHeader from "../components/common/PageHeader.vue";
import ContentFailureAlert from "../components/content/ContentFailureAlert.vue";
import ContentPagination from "../components/content/ContentPagination.vue";
import ContentSourceBadge from "../components/content/ContentSourceBadge.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../components/ui/alert";
import { Button } from "../components/ui/button";
import { Field, FieldLabel } from "../components/ui/field";
import { ScrollArea, ScrollBar } from "../components/ui/scroll-area";
import { Skeleton } from "../components/ui/skeleton";

interface DiscoverFilters {
  platformId?: string;
  pluginId?: string;
}

interface DiscoverAttempt {
  filters: DiscoverFilters;
  cursor: string | undefined;
  history: Array<string | undefined>;
  refreshCategories: boolean;
  activate: boolean;
}

const router = useRouter();
const platformId = ref("");
const pluginId = ref("");
const plugins = ref<PluginItem[]>([]);
const state = ref<ContentEnvelope | null>(null);
const categories = ref<CategoriesResponse | null>(null);
const requestError = ref("");
const loading = ref(false);
const cursorHistory = ref<Array<string | undefined>>([]);
const currentCursor = ref<string | undefined>();
const activeFilters = ref<DiscoverFilters>({});
const lastAttempt = ref<DiscoverAttempt | null>(null);

const contentPlugins = computed(() =>
  plugins.value.filter(
    (plugin) =>
      plugin.pluginType === "content" &&
      plugin.enabled &&
      plugin.discoverEnabled !== false &&
      plugin.capabilities.includes("content.discover"),
  ),
);
const platforms = computed(() => {
  const values = new Set(
    contentPlugins.value.map((plugin) => plugin.platformId),
  );
  return [...values].sort();
});
const pluginOptions = computed(() =>
  contentPlugins.value.filter(
    (plugin) =>
      platformId.value.length === 0 ||
      plugin.platformId === platformId.value,
  ),
);
const failures = computed(() => {
  const unique = new Map<string, ContentFailure>();
  for (const failure of [
    ...(state.value?.failures ?? []),
    ...(categories.value?.failures ?? []),
  ]) {
    unique.set(
      `${failure.source.platformId}:${failure.source.pluginId}:${failure.code}`,
      failure,
    );
  }
  return [...unique.values()];
});
const hasContent = computed(
  () =>
    (state.value?.sections.length ?? 0) > 0 ||
    (categories.value?.items?.length ?? 0) > 0,
);
const noContent = computed(
  () =>
    state.value !== null &&
    !state.value.emptyState &&
    !hasContent.value &&
    failures.value.length === 0,
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

function safeRequestError(error: unknown) {
  return error instanceof ApiError
    ? `${error.code}: 无法读取发现内容`
    : "无法读取发现内容";
}

async function loadPlugins() {
  try {
    const response = await api.plugins();
    plugins.value = Array.isArray(response.items) ? response.items : [];
  } catch {
    plugins.value = [];
  }
}

async function executeDiscover(
  filters: DiscoverFilters,
  cursor: string | undefined,
  history: Array<string | undefined>,
  refreshCategories: boolean,
  activate: boolean,
) {
  lastAttempt.value = {
    filters: { ...filters },
    cursor,
    history: [...history],
    refreshCategories,
    activate,
  };
  loading.value = true;
  requestError.value = "";
  try {
    const options = { ...filters, cursor };
    const [discoverResponse, categoriesResponse] = await Promise.all([
      api.discover(options),
      refreshCategories
        ? api.categories(filters)
        : Promise.resolve(categories.value),
    ]);
    state.value = discoverResponse;
    if (categoriesResponse) {
      categories.value = categoriesResponse;
    }
    if (activate) {
      activeFilters.value = { ...filters };
    }
    currentCursor.value = cursor;
    cursorHistory.value = history;
  } catch (error) {
    requestError.value = safeRequestError(error);
  } finally {
    loading.value = false;
  }
}

async function initialize() {
  loading.value = true;
  await loadPlugins();
  await executeDiscover({}, undefined, [], true, true);
}

async function applyFilters() {
  await executeDiscover(
    {
      platformId: platformId.value || undefined,
      pluginId: pluginId.value || undefined,
    },
    undefined,
    [],
    true,
    true,
  );
}

async function nextPage(cursor: string) {
  await executeDiscover(
    activeFilters.value,
    cursor,
    [...cursorHistory.value, currentCursor.value],
    false,
    false,
  );
}

async function previousPage() {
  if (cursorHistory.value.length === 0) {
    return;
  }
  const history = [...cursorHistory.value];
  const cursor = history.pop();
  await executeDiscover(
    activeFilters.value,
    cursor,
    history,
    false,
    false,
  );
}

async function retry() {
  if (!lastAttempt.value) {
    await initialize();
    return;
  }
  await executeDiscover(
    lastAttempt.value.filters,
    lastAttempt.value.cursor,
    lastAttempt.value.history,
    lastAttempt.value.refreshCategories,
    lastAttempt.value.activate,
  );
}

function itemTag(item: ContentItem) {
  return item.resourceType === "album" ? RouterLink : "article";
}

function itemRoute(item: ContentItem, source: ContentSource) {
  return item.resourceType === "album"
    ? {
        name: "album",
        query: {
          pluginId: source.pluginId,
          resourceId: item.resourceId,
        },
      }
    : undefined;
}

function swatchClass(layout: DiscoverLayout) {
  switch (layout) {
    case "hero-carousel":
      return "bg-emerald-700";
    case "album-grid":
      return "bg-sky-700";
    case "horizontal-list":
      return "bg-amber-600";
    case "ranked-list":
      return "bg-rose-700";
    case "category-grid":
      return "bg-zinc-700";
  }
}

onMounted(initialize);
</script>

<template>
  <section class="grid min-w-0 gap-5">
    <PageHeader
      title="发现"
      description="浏览内容插件提供的频道、分类和专辑。"
    />

    <form
      class="grid min-w-0 gap-3 sm:grid-cols-2 lg:grid-cols-[12rem_minmax(14rem,18rem)_auto]"
      @submit.prevent="applyFilters"
    >
      <Field class="min-w-0">
        <FieldLabel for="discover-platform">平台</FieldLabel>
        <select
          id="discover-platform"
          v-model="platformId"
          name="platform"
          class="h-9 w-full min-w-0 rounded-[5px] border border-input bg-transparent px-2.5 text-sm outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
          :disabled="loading"
        >
          <option value="">全部平台</option>
          <option
            v-for="platform in platforms"
            :key="platform"
            :value="platform"
          >
            {{ platform }}
          </option>
        </select>
      </Field>

      <Field class="min-w-0">
        <FieldLabel for="discover-plugin">插件</FieldLabel>
        <select
          id="discover-plugin"
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

      <div class="flex items-end sm:col-span-2 lg:col-span-1">
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
          <Compass v-else aria-hidden="true" />
          {{ loading ? "加载中" : "应用筛选" }}
        </Button>
      </div>
    </form>

    <Alert v-if="requestError" variant="destructive">
      <AlertTitle>发现内容加载失败</AlertTitle>
      <AlertDescription>{{ requestError }}</AlertDescription>
      <Button
        class="mt-2 w-fit"
        type="button"
        variant="outline"
        size="sm"
        aria-label="重试"
        :disabled="loading"
        @click="retry"
      >
        <RotateCcw aria-hidden="true" />
        重试
      </Button>
    </Alert>

    <div
      v-if="loading && !state"
      class="grid min-h-72 gap-4"
      role="status"
      aria-label="正在加载发现内容"
    >
      <Skeleton class="h-24 w-full" />
      <Skeleton class="h-48 w-full" />
      <Skeleton class="h-32 w-full" />
    </div>

    <template v-else-if="state">
      <ContentFailureAlert :failures="failures" />

      <EmptyState
        v-if="state.emptyState && !hasContent"
        class="min-h-[420px] border-0 bg-transparent"
        :title="state.emptyState.title"
        description="安装内容插件后，发现频道会显示在这里。"
        :primary-label="state.emptyState.actionLabel"
        @primary="router.push('/plugins')"
      >
        <template #icon>
          <Compass aria-hidden="true" />
        </template>
      </EmptyState>

      <EmptyState
        v-else-if="noContent"
        class="min-h-72 border-0 bg-transparent"
        title="暂无发现内容"
        description="调整平台或插件筛选条件后重试。"
      >
        <template #icon>
          <Compass aria-hidden="true" />
        </template>
      </EmptyState>

      <template v-else>
        <section
          v-if="categories?.items?.length"
          data-categories
          class="grid min-w-0 gap-3 border-b border-border pb-5"
        >
          <div class="flex items-center gap-2">
            <Tags class="size-4 text-muted-foreground" aria-hidden="true" />
            <h2 class="text-sm font-semibold">分类</h2>
          </div>
          <ScrollArea class="min-w-0 whitespace-nowrap">
            <div class="flex w-max gap-2 pb-3">
              <article
                v-for="category in categories.items"
                :key="`${category.source.pluginId}:${category.item.resourceId}`"
                class="grid w-56 gap-1 rounded-[6px] border border-border bg-card px-3 py-2.5 whitespace-normal"
              >
                <strong class="break-words text-sm">
                  {{ category.item.title }}
                </strong>
                <span
                  v-if="category.item.description"
                  class="line-clamp-2 break-words text-xs text-muted-foreground"
                >
                  {{ category.item.description }}
                </span>
                <span class="break-words text-xs text-muted-foreground">
                  {{ category.source.pluginName }}
                  {{ category.source.pluginVersion }}
                </span>
              </article>
            </div>
            <ScrollBar orientation="horizontal" />
          </ScrollArea>
        </section>

        <section
          v-for="entry in state.sections"
          :key="`${entry.source.pluginId}:${entry.section.id}`"
          :data-discover-layout="entry.section.layout"
          class="grid min-w-0 gap-3 border-b border-border pb-5 last:border-b-0"
        >
          <div
            class="flex min-w-0 flex-wrap items-start justify-between gap-2"
          >
            <div class="min-w-0">
              <h2 class="break-words text-base font-semibold">
                {{ entry.section.title }}
              </h2>
              <ContentSourceBadge
                class="mt-1.5"
                :source="entry.source"
              />
            </div>
          </div>

          <ScrollArea
            v-if="entry.section.layout === 'hero-carousel'"
            class="min-w-0 whitespace-nowrap"
          >
            <div class="flex w-max gap-3 pb-3">
              <component
                :is="itemTag(item)"
                v-for="item in entry.section.items"
                :key="item.resourceId"
                :to="itemRoute(item, entry.source)"
                :aria-label="
                  item.resourceType === 'album'
                    ? `打开 ${item.title}`
                    : undefined
                "
                :data-resource-id="item.resourceId"
                class="grid min-h-44 w-[min(78vw,34rem)] min-w-0 grid-cols-[6rem_minmax(0,1fr)] items-center gap-4 rounded-[6px] border border-border bg-card p-4 text-left whitespace-normal outline-none transition-colors hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                <span
                  class="flex aspect-square items-center justify-center rounded-[5px] text-white"
                  :class="swatchClass(entry.section.layout)"
                >
                  <Disc3 class="size-8" aria-hidden="true" />
                </span>
                <span class="grid min-w-0 gap-1.5">
                  <strong class="break-words text-base">{{ item.title }}</strong>
                  <span
                    v-if="item.subtitle"
                    class="break-words text-sm text-muted-foreground"
                  >
                    {{ item.subtitle }}
                  </span>
                  <span
                    v-if="item.description"
                    class="line-clamp-2 break-words text-sm text-muted-foreground"
                  >
                    {{ item.description }}
                  </span>
                </span>
              </component>
            </div>
            <ScrollBar orientation="horizontal" />
          </ScrollArea>

          <div
            v-else-if="entry.section.layout === 'album-grid'"
            class="grid min-w-0 grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-5"
          >
            <component
              :is="itemTag(item)"
              v-for="item in entry.section.items"
              :key="item.resourceId"
              :to="itemRoute(item, entry.source)"
              :aria-label="
                item.resourceType === 'album'
                  ? `打开 ${item.title}`
                  : undefined
              "
              :data-resource-id="item.resourceId"
              class="grid min-w-0 gap-3 rounded-[6px] border border-border bg-card p-3 text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/50"
            >
              <span
                class="flex aspect-[4/3] items-center justify-center rounded-[5px] text-white"
                :class="swatchClass(entry.section.layout)"
              >
                <Disc3 class="size-7" aria-hidden="true" />
              </span>
              <span class="grid min-w-0 gap-1">
                <strong class="break-words text-sm">{{ item.title }}</strong>
                <span
                  v-if="item.subtitle"
                  class="break-words text-xs text-muted-foreground"
                >
                  {{ item.subtitle }}
                </span>
              </span>
            </component>
          </div>

          <ScrollArea
            v-else-if="entry.section.layout === 'horizontal-list'"
            class="min-w-0 whitespace-nowrap"
          >
            <div class="flex w-max gap-3 pb-3">
              <component
                :is="itemTag(item)"
                v-for="item in entry.section.items"
                :key="item.resourceId"
                :to="itemRoute(item, entry.source)"
                :aria-label="
                  item.resourceType === 'album'
                    ? `打开 ${item.title}`
                    : undefined
                "
                :data-resource-id="item.resourceId"
                class="grid w-64 grid-cols-[4rem_minmax(0,1fr)] items-center gap-3 rounded-[6px] border border-border bg-card p-3 text-left whitespace-normal outline-none transition-colors hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                <span
                  class="flex aspect-square items-center justify-center rounded-[5px] text-white"
                  :class="swatchClass(entry.section.layout)"
                >
                  <Disc3 class="size-5" aria-hidden="true" />
                </span>
                <span class="grid min-w-0 gap-1">
                  <strong class="break-words text-sm">{{ item.title }}</strong>
                  <span
                    v-if="item.subtitle"
                    class="break-words text-xs text-muted-foreground"
                  >
                    {{ item.subtitle }}
                  </span>
                </span>
              </component>
            </div>
            <ScrollBar orientation="horizontal" />
          </ScrollArea>

          <ol
            v-else-if="entry.section.layout === 'ranked-list'"
            class="grid min-w-0"
          >
            <li
              v-for="(item, index) in entry.section.items"
              :key="item.resourceId"
              class="border-b border-border last:border-b-0"
            >
              <component
                :is="itemTag(item)"
                :to="itemRoute(item, entry.source)"
                :aria-label="
                  item.resourceType === 'album'
                    ? `打开 ${item.title}`
                    : undefined
                "
                :data-resource-id="item.resourceId"
                class="grid w-full min-w-0 grid-cols-[2.5rem_minmax(0,1fr)_auto] items-center gap-3 px-1 py-3 text-left outline-none transition-colors hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/50"
              >
                <span class="text-lg font-semibold text-muted-foreground">
                  {{ index + 1 }}
                </span>
                <span class="grid min-w-0 gap-0.5">
                  <strong class="break-words text-sm">{{ item.title }}</strong>
                  <span
                    v-if="item.subtitle"
                    class="break-words text-xs text-muted-foreground"
                  >
                    {{ item.subtitle }}
                  </span>
                </span>
                <ArrowUpRight
                  v-if="item.resourceType === 'album'"
                  class="size-4 text-muted-foreground"
                  aria-hidden="true"
                />
              </component>
            </li>
          </ol>

          <div
            v-else
            class="grid min-w-0 gap-2 sm:grid-cols-2 lg:grid-cols-3"
          >
            <article
              v-for="item in entry.section.items"
              :key="item.resourceId"
              :data-resource-id="item.resourceId"
              class="grid min-w-0 grid-cols-[2.5rem_minmax(0,1fr)] items-center gap-3 rounded-[6px] border border-border bg-card p-3"
            >
              <span
                class="flex aspect-square items-center justify-center rounded-[5px] text-white"
                :class="swatchClass(entry.section.layout)"
              >
                <Tags class="size-4" aria-hidden="true" />
              </span>
              <span class="grid min-w-0 gap-0.5">
                <strong class="break-words text-sm">{{ item.title }}</strong>
                <span
                  v-if="item.description"
                  class="line-clamp-2 break-words text-xs text-muted-foreground"
                >
                  {{ item.description }}
                </span>
              </span>
            </article>
          </div>
        </section>

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
