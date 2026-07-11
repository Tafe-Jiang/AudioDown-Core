<script setup lang="ts">
import { ChevronRight } from "@lucide/vue";

import type { SourcedContentItem } from "@/api/client";
import ContentSourceBadge from "./ContentSourceBadge.vue";

const props = defineProps<{
  result: SourcedContentItem;
}>();

const emit = defineEmits<{
  open: [result: SourcedContentItem];
}>();

function open() {
  emit("open", props.result);
}
</script>

<template>
  <article
    role="button"
    tabindex="0"
    class="grid min-w-0 cursor-pointer gap-2 border-b border-border px-1 py-3 outline-none transition-colors last:border-b-0 hover:bg-muted/50 focus-visible:ring-3 focus-visible:ring-ring/50 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center"
    @click="open"
    @keydown.enter.prevent="open"
    @keydown.space.prevent="open"
  >
    <div class="grid min-w-0 gap-1.5">
      <div class="min-w-0">
        <strong class="block break-words text-sm">
          {{ result.item.title }}
        </strong>
        <span
          v-if="result.item.subtitle"
          class="block break-words text-xs text-muted-foreground"
        >
          {{ result.item.subtitle }}
        </span>
      </div>
      <p
        v-if="result.item.description"
        class="line-clamp-2 break-words text-sm text-muted-foreground"
      >
        {{ result.item.description }}
      </p>
      <ContentSourceBadge :source="result.source" />
    </div>
    <ChevronRight
      class="hidden size-4 text-muted-foreground sm:block"
      aria-hidden="true"
    />
  </article>
</template>
