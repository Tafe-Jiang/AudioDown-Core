<script setup lang="ts">
import { computed, ref, watch } from "vue";
import {
  ArrowLeft,
  Disc3,
  ListMusic,
  LoaderCircle,
  RotateCcw,
} from "@lucide/vue";
import { useRoute, useRouter } from "vue-router";

import {
  ApiError,
  api,
  type AlbumResponse,
  type TracksResponse,
} from "../api/client";
import PageHeader from "../components/common/PageHeader.vue";
import ContentPagination from "../components/content/ContentPagination.vue";
import ContentSourceBadge from "../components/content/ContentSourceBadge.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../components/ui/alert";
import { Button } from "../components/ui/button";
import { Skeleton } from "../components/ui/skeleton";

const route = useRoute();
const router = useRouter();
const albumState = ref<AlbumResponse | null>(null);
const tracksState = ref<TracksResponse | null>(null);
const albumError = ref("");
const tracksError = ref("");
const loadingAlbum = ref(false);
const loadingTracks = ref(false);
const cursorHistory = ref<Array<string | undefined>>([]);
const currentCursor = ref<string | undefined>();
const lastTrackAttempt = ref<{
  cursor: string | undefined;
  history: Array<string | undefined>;
} | null>(null);

const pluginId = computed(() => queryValue(route.query.pluginId));
const resourceId = computed(() => queryValue(route.query.resourceId));
const busy = computed(() => loadingAlbum.value || loadingTracks.value);

function queryValue(value: unknown) {
  if (Array.isArray(value)) {
    return typeof value[0] === "string" ? value[0] : "";
  }
  return typeof value === "string" ? value : "";
}

function safeAlbumError(error: unknown) {
  if (error instanceof ApiError) {
    if (error.code === "RESOURCE_NOT_FOUND") {
      return "RESOURCE_NOT_FOUND：未找到该专辑";
    }
    return `${error.code}：无法读取专辑详情`;
  }
  return "无法读取专辑详情";
}

function safeTracksError(error: unknown) {
  return error instanceof ApiError
    ? `${error.code}：无法读取曲目`
    : "无法读取曲目";
}

async function loadTracks(
  cursor: string | undefined,
  history: Array<string | undefined>,
) {
  if (!pluginId.value || !resourceId.value) {
    return;
  }
  lastTrackAttempt.value = {
    cursor,
    history: [...history],
  };
  loadingTracks.value = true;
  tracksError.value = "";
  try {
    const response = await api.tracks(
      pluginId.value,
      resourceId.value,
      cursor,
    );
    tracksState.value = response;
    currentCursor.value = cursor;
    cursorHistory.value = history;
  } catch (error) {
    tracksError.value = safeTracksError(error);
  } finally {
    loadingTracks.value = false;
  }
}

async function loadAlbum() {
  albumState.value = null;
  tracksState.value = null;
  albumError.value = "";
  tracksError.value = "";
  cursorHistory.value = [];
  currentCursor.value = undefined;
  lastTrackAttempt.value = null;

  if (!pluginId.value || !resourceId.value) {
    albumError.value =
      "专辑链接无效：缺少来源插件或资源标识。";
    return;
  }

  loadingAlbum.value = true;
  try {
    albumState.value = await api.album(
      pluginId.value,
      resourceId.value,
    );
  } catch (error) {
    albumError.value = safeAlbumError(error);
    return;
  } finally {
    loadingAlbum.value = false;
  }

  await loadTracks(undefined, []);
}

async function nextPage(cursor: string) {
  await loadTracks(cursor, [
    ...cursorHistory.value,
    currentCursor.value,
  ]);
}

async function previousPage() {
  if (cursorHistory.value.length === 0) {
    return;
  }
  const history = [...cursorHistory.value];
  const cursor = history.pop();
  await loadTracks(cursor, history);
}

async function retry() {
  if (albumState.value && tracksError.value) {
    await loadTracks(
      lastTrackAttempt.value?.cursor,
      lastTrackAttempt.value?.history ?? cursorHistory.value,
    );
    return;
  }
  await loadAlbum();
}

function duration(seconds?: number) {
  if (seconds === undefined) {
    return "—";
  }
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(seconds % 60).padStart(2, "0")}`;
}

watch(
  () => route.fullPath,
  () => {
    void loadAlbum();
  },
  { immediate: true },
);
</script>

<template>
  <section class="grid min-w-0 gap-5">
    <PageHeader
      :title="albumState?.album.title ?? '专辑详情'"
      :description="
        albumState?.album.creator
          ? `创作者：${albumState.album.creator}`
          : '查看来源插件提供的专辑和曲目。'
      "
    >
      <template #actions>
        <Button
          type="button"
          variant="outline"
          aria-label="返回发现"
          @click="router.push('/discover')"
        >
          <ArrowLeft aria-hidden="true" />
          返回
        </Button>
      </template>
    </PageHeader>

    <div
      v-if="loadingAlbum && !albumState"
      class="grid min-h-72 gap-4"
      role="status"
      aria-label="正在加载专辑"
    >
      <Skeleton class="h-36 w-full" />
      <Skeleton class="h-12 w-full" />
      <Skeleton class="h-12 w-full" />
    </div>

    <Alert v-else-if="albumError" variant="destructive">
      <AlertTitle>无法打开专辑</AlertTitle>
      <AlertDescription>{{ albumError }}</AlertDescription>
      <Button
        v-if="pluginId && resourceId"
        class="mt-2 w-fit"
        type="button"
        variant="outline"
        size="sm"
        aria-label="重试"
        :disabled="busy"
        @click="retry"
      >
        <RotateCcw aria-hidden="true" />
        重试
      </Button>
    </Alert>

    <template v-else-if="albumState">
      <section
        class="grid min-w-0 grid-cols-[6rem_minmax(0,1fr)] items-start gap-4 border-b border-border pb-5 md:grid-cols-[9rem_minmax(0,1fr)]"
      >
        <div
          class="flex aspect-square items-center justify-center rounded-[6px] bg-emerald-700 text-white"
          aria-hidden="true"
        >
          <Disc3 class="size-12" />
        </div>
        <div class="grid min-w-0 gap-3">
          <div class="grid min-w-0 gap-1">
            <p
              v-if="albumState.album.description"
              class="break-all text-sm leading-6 text-muted-foreground"
            >
              {{ albumState.album.description }}
            </p>
            <p
              v-if="albumState.album.trackCount !== undefined"
              class="text-sm text-muted-foreground"
            >
              {{ albumState.album.trackCount }} 首曲目
            </p>
          </div>
          <ContentSourceBadge :source="albumState.source" />
          <span
            class="min-w-0 break-all text-xs text-muted-foreground"
          >
            资源标识：{{ albumState.album.resourceId }}
          </span>
        </div>
      </section>

      <section class="grid min-w-0 gap-3">
        <div class="flex min-w-0 items-center justify-between gap-3">
          <div class="flex min-w-0 items-center gap-2">
            <ListMusic
              class="size-4 text-muted-foreground"
              aria-hidden="true"
            />
            <h2 class="text-base font-semibold">曲目</h2>
          </div>
          <LoaderCircle
            v-if="loadingTracks"
            class="size-4 animate-spin text-muted-foreground"
            aria-label="正在加载曲目"
          />
        </div>

        <Alert v-if="tracksError" variant="destructive">
          <AlertTitle>曲目加载失败</AlertTitle>
          <AlertDescription>{{ tracksError }}</AlertDescription>
          <Button
            class="mt-2 w-fit"
            type="button"
            variant="outline"
            size="sm"
            aria-label="重试曲目"
            :disabled="busy"
            @click="retry"
          >
            <RotateCcw aria-hidden="true" />
            重试
          </Button>
        </Alert>

        <ol
          v-if="tracksState"
          class="grid min-w-0"
          aria-label="曲目列表"
        >
          <li
            v-for="(track, index) in tracksState.items"
            :key="track.resourceId"
            :data-track-id="track.resourceId"
            class="grid min-w-0 grid-cols-[2.5rem_minmax(0,1fr)_auto] items-center gap-3 border-b border-border px-1 py-3 last:border-b-0"
          >
            <span class="text-sm text-muted-foreground">
              {{ track.sequence ?? index + 1 }}
            </span>
            <span class="grid min-w-0 gap-0.5">
              <strong class="break-words text-sm">{{ track.title }}</strong>
              <span
                v-if="track.subtitle"
                class="break-words text-xs text-muted-foreground"
              >
                {{ track.subtitle }}
              </span>
            </span>
            <time
              class="text-sm tabular-nums text-muted-foreground"
              :datetime="
                track.durationSeconds === undefined
                  ? undefined
                  : `PT${track.durationSeconds}S`
              "
            >
              {{ duration(track.durationSeconds) }}
            </time>
          </li>
        </ol>

        <ContentPagination
          v-if="tracksState"
          :has-previous="cursorHistory.length > 0"
          :next-cursor="tracksState.nextCursor"
          :busy="loadingTracks"
          @previous="previousPage"
          @next="nextPage"
        />
      </section>
    </template>
  </section>
</template>
