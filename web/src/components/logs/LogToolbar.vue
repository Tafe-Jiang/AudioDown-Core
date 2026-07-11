<script setup lang="ts">
import { computed } from "vue";
import { RefreshCw, Search, X } from "@lucide/vue";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const props = defineProps<{
  level: string;
  component: string;
  query: string;
  levels: string[];
  components: string[];
  refreshing: boolean;
}>();

const emit = defineEmits<{
  "update:level": [level: string];
  "update:component": [component: string];
  "update:query": [query: string];
  refresh: [];
  clear: [];
}>();

const hasFilters = computed(
  () =>
    props.level !== "all" ||
    props.component !== "all" ||
    props.query.length > 0,
);
</script>

<template>
  <div
    class="flex min-w-0 flex-col gap-2 border-b border-border pb-4 lg:flex-row lg:items-center"
  >
    <div class="relative min-w-0 flex-1 lg:max-w-sm">
      <Search
        class="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
        aria-hidden="true"
      />
      <Input
        :model-value="query"
        name="log-query"
        type="search"
        class="pl-8"
        placeholder="筛选消息、组件或插件"
        aria-label="筛选日志文本"
        @update:model-value="emit('update:query', String($event))"
      />
    </div>

    <div class="grid grid-cols-2 gap-2 sm:flex">
      <Select
        :model-value="level"
        @update:model-value="emit('update:level', String($event))"
      >
        <SelectTrigger aria-label="筛选日志级别" class="w-full sm:w-32">
          <SelectValue placeholder="全部级别" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部级别</SelectItem>
          <SelectItem v-for="item in levels" :key="item" :value="item">
            {{ item }}
          </SelectItem>
        </SelectContent>
      </Select>

      <Select
        :model-value="component"
        @update:model-value="emit('update:component', String($event))"
      >
        <SelectTrigger aria-label="筛选日志组件" class="w-full sm:w-40">
          <SelectValue placeholder="全部组件" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">全部组件</SelectItem>
          <SelectItem v-for="item in components" :key="item" :value="item">
            {{ item }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <div class="flex items-center justify-end gap-1">
      <Button
        type="button"
        variant="ghost"
        size="icon-sm"
        aria-label="清除日志筛选"
        title="清除日志筛选"
        :disabled="!hasFilters"
        @click="emit('clear')"
      >
        <X aria-hidden="true" />
      </Button>
      <Button
        type="button"
        variant="outline"
        size="sm"
        aria-label="刷新日志"
        :disabled="refreshing"
        @click="emit('refresh')"
      >
        <RefreshCw
          :class="{ 'animate-spin': refreshing }"
          aria-hidden="true"
        />
        {{ refreshing ? "刷新中" : "刷新" }}
      </Button>
    </div>
  </div>
</template>
