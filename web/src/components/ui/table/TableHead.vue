<script setup lang="ts">
import type { HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

defineProps<{
  class?: HTMLAttributes["class"];
}>();
</script>

<template>
  <th
    data-slot="table-head"
    :class="
      cn(
        'h-10 px-2 text-left align-middle font-medium text-foreground whitespace-nowrap',
        $props.class,
      )
    "
  >
    <slot />
  </th>
</template>
