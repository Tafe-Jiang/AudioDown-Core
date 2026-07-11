<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ScrollText, TriangleAlert } from "@lucide/vue";

import { api, type StructuredLog } from "../api/client";
import PageHeader from "../components/common/PageHeader.vue";
import LogDetailsSheet from "../components/logs/LogDetailsSheet.vue";
import LogTable from "../components/logs/LogTable.vue";
import LogToolbar from "../components/logs/LogToolbar.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../components/ui/alert";
import { Button } from "../components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "../components/ui/empty";

const logs = ref<StructuredLog[] | null>(null);
const loading = ref(true);
const refreshing = ref(false);
const error = ref("");
const level = ref("all");
const component = ref("all");
const query = ref("");
const selectedLog = ref<StructuredLog | null>(null);

const levels = computed(() =>
  [...new Set((logs.value ?? []).map((log) => log.level))].sort(),
);
const components = computed(() =>
  [...new Set((logs.value ?? []).map((log) => log.component))].sort(),
);
const filteredLogs = computed(() => {
  const normalizedQuery = query.value.trim().toLocaleLowerCase();
  return (logs.value ?? []).filter((log) => {
    if (level.value !== "all" && log.level !== level.value) {
      return false;
    }
    if (component.value !== "all" && log.component !== component.value) {
      return false;
    }
    if (!normalizedQuery) {
      return true;
    }
    return [
      log.message,
      log.component,
      log.pluginId ?? "",
      log.level,
    ].some((value) => value.toLocaleLowerCase().includes(normalizedQuery));
  });
});
const hasFilters = computed(
  () =>
    level.value !== "all" ||
    component.value !== "all" ||
    query.value.length > 0,
);

function clearFilters() {
  level.value = "all";
  component.value = "all";
  query.value = "";
}

async function loadLogs() {
  const initial = logs.value === null;
  if (initial) {
    loading.value = true;
  } else {
    refreshing.value = true;
  }
  error.value = "";
  try {
    const result = await api.logs();
    logs.value = result.items;
  } catch {
    error.value = "无法读取日志，请检查 Core 状态后重试";
  } finally {
    loading.value = false;
    refreshing.value = false;
  }
}

onMounted(loadLogs);
</script>

<template>
  <section class="grid min-w-0 gap-5">
    <PageHeader
      title="日志"
      description="查看 Core、Supervisor 和插件产生的结构化事件。"
    />

    <LogToolbar
      :level="level"
      :component="component"
      :query="query"
      :levels="levels"
      :components="components"
      :refreshing="loading || refreshing"
      @update:level="level = $event"
      @update:component="component = $event"
      @update:query="query = $event"
      @clear="clearFilters"
      @refresh="loadLogs"
    />

    <Alert v-if="error" variant="destructive">
      <TriangleAlert aria-hidden="true" />
      <AlertTitle>日志读取失败</AlertTitle>
      <AlertDescription>{{ error }}</AlertDescription>
    </Alert>

    <LogTable
      v-if="loading || (logs && filteredLogs.length > 0)"
      :items="filteredLogs"
      :loading="loading"
      @select="selectedLog = $event"
    />

    <Empty
      v-else-if="logs"
      data-log-empty
      class="min-h-40 border border-dashed border-border"
    >
      <EmptyHeader>
        <EmptyMedia variant="icon">
          <ScrollText aria-hidden="true" />
        </EmptyMedia>
        <EmptyTitle>
          {{ hasFilters ? "没有匹配日志" : "暂无结构化日志" }}
        </EmptyTitle>
        <EmptyDescription>
          {{
            hasFilters
              ? "调整或清除当前筛选条件。"
              : "新的 Core、Supervisor 和插件事件会显示在这里。"
          }}
        </EmptyDescription>
      </EmptyHeader>
      <EmptyContent v-if="hasFilters">
        <Button type="button" variant="outline" @click="clearFilters">
          清除筛选
        </Button>
      </EmptyContent>
    </Empty>

    <LogDetailsSheet
      :open="selectedLog !== null"
      :log="selectedLog"
      @update:open="!$event && (selectedLog = null)"
    />
  </section>
</template>
