<script setup lang="ts">
import { computed } from "vue";

import type { StructuredLog } from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { Skeleton } from "@/components/ui/skeleton";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const props = defineProps<{
  items: StructuredLog[];
  loading: boolean;
}>();

const emit = defineEmits<{
  select: [log: StructuredLog];
}>();

const rows = computed(() => props.items);

function formatTimestamp(value: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function levelClass(level: string) {
  switch (level) {
    case "error":
      return "border-status-danger/25 bg-status-danger/10 text-status-danger";
    case "warn":
      return "border-status-warning/25 bg-status-warning/10 text-status-warning";
    case "info":
      return "border-status-success/25 bg-status-success/10 text-status-success";
    default:
      return "border-border bg-muted text-muted-foreground";
  }
}

function open(log: StructuredLog) {
  emit("select", log);
}
</script>

<template>
  <div
    v-if="loading"
    data-log-skeleton
    class="overflow-hidden rounded-md border border-border"
    role="status"
    aria-label="正在读取日志"
  >
    <div
      class="grid grid-cols-[minmax(10rem,0.8fr)_5rem_minmax(8rem,0.7fr)_minmax(16rem,2fr)] gap-3 border-b border-border bg-muted/40 px-3 py-2 text-xs font-medium text-muted-foreground"
    >
      <span>时间</span>
      <span>级别</span>
      <span>组件</span>
      <span>消息</span>
    </div>
    <div
      v-for="index in 3"
      :key="index"
      data-log-skeleton-row
      class="grid grid-cols-[minmax(10rem,0.8fr)_5rem_minmax(8rem,0.7fr)_minmax(16rem,2fr)] gap-3 border-b border-border px-3 py-3 last:border-b-0"
    >
      <Skeleton class="h-4 w-32" />
      <Skeleton class="h-4 w-12" />
      <Skeleton class="h-4 w-24" />
      <Skeleton class="h-4 w-full" />
    </div>
  </div>

  <template v-else>
    <div
      data-desktop-log-table
      class="hidden overflow-hidden rounded-md border border-border md:block"
    >
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>时间</TableHead>
            <TableHead>级别</TableHead>
            <TableHead>组件</TableHead>
            <TableHead>消息</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow
            v-for="log in rows"
            :key="log.id"
            :data-desktop-log-row="log.id"
            class="cursor-pointer"
            @click="open(log)"
          >
            <TableCell class="whitespace-nowrap">
              <time :datetime="log.timestamp">
                {{ formatTimestamp(log.timestamp) }}
              </time>
            </TableCell>
            <TableCell>
              <Badge variant="outline" :class="levelClass(log.level)">
                {{ log.level }}
              </Badge>
            </TableCell>
            <TableCell>{{ log.component }}</TableCell>
            <TableCell>
              <button
                type="button"
                class="line-clamp-2 w-full text-left"
                :aria-label="`查看日志详情：${log.message}`"
                @click.stop="open(log)"
              >
                {{ log.message }}
              </button>
            </TableCell>
          </TableRow>
        </TableBody>
      </Table>
    </div>

    <div class="grid min-w-0 md:hidden">
      <article
        v-for="log in rows"
        :key="`mobile-${log.id}`"
        :data-mobile-log="log.id"
        class="grid min-w-0 cursor-pointer gap-2 border-b border-border py-3"
        role="button"
        tabindex="0"
        @click="open(log)"
        @keydown.enter="open(log)"
        @keydown.space.prevent="open(log)"
      >
        <header class="flex min-w-0 items-start justify-between gap-3">
          <time
            :datetime="log.timestamp"
            class="min-w-0 break-words text-xs text-muted-foreground"
          >
            {{ formatTimestamp(log.timestamp) }}
          </time>
          <Badge
            variant="outline"
            class="shrink-0"
            :class="levelClass(log.level)"
          >
            {{ log.level }}
          </Badge>
        </header>
        <strong class="break-words text-sm font-medium">
          {{ log.message }}
        </strong>
        <span class="break-words text-xs text-muted-foreground">
          {{ log.component }}
        </span>
      </article>
    </div>
  </template>
</template>
