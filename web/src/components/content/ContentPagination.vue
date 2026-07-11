<script setup lang="ts">
import { ChevronLeft, ChevronRight } from "@lucide/vue";

import { Button } from "@/components/ui/button";

const props = defineProps<{
  hasPrevious: boolean;
  nextCursor: string | null;
  busy: boolean;
}>();

const emit = defineEmits<{
  previous: [];
  next: [cursor: string];
}>();

function next() {
  if (props.nextCursor) {
    emit("next", props.nextCursor);
  }
}
</script>

<template>
  <nav
    class="flex items-center justify-end gap-2"
    aria-label="内容分页"
  >
    <Button
      type="button"
      variant="outline"
      :disabled="busy || !hasPrevious"
      aria-label="上一页"
      @click="emit('previous')"
    >
      <ChevronLeft aria-hidden="true" />
      上一页
    </Button>
    <Button
      type="button"
      variant="outline"
      :disabled="busy || !nextCursor"
      aria-label="下一页"
      @click="next"
    >
      下一页
      <ChevronRight aria-hidden="true" />
    </Button>
  </nav>
</template>
