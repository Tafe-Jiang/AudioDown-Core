<script setup lang="ts">
import type { SourcedContentItem } from "@/api/client";
import { Skeleton } from "@/components/ui/skeleton";
import ContentItemRow from "./ContentItemRow.vue";

withDefaults(
  defineProps<{
    items: SourcedContentItem[];
    loading?: boolean;
  }>(),
  {
    loading: false,
  },
);

defineEmits<{
  open: [result: SourcedContentItem];
}>();
</script>

<template>
  <div data-content-grid class="min-h-48 min-w-0">
    <div v-if="loading" class="grid gap-3 py-2" aria-label="正在加载内容">
      <Skeleton v-for="index in 3" :key="index" class="h-16 w-full" />
    </div>
    <div v-else class="min-w-0">
      <ContentItemRow
        v-for="result in items"
        :key="`${result.source.pluginId}:${result.item.resourceType}:${result.item.resourceId}`"
        :result="result"
        @open="$emit('open', $event)"
      />
    </div>
  </div>
</template>
