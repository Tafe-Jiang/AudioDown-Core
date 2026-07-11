<script setup lang="ts">
import type { HTMLAttributes } from "vue";

import { cn } from "@/lib/utils";

withDefaults(
  defineProps<{
    class?: HTMLAttributes["class"];
    colspan?: number;
  }>(),
  {
    colspan: 1,
  },
);
</script>

<template>
  <tr data-slot="table-empty">
    <td
      :colspan="colspan"
      :class="
        cn(
          'h-24 px-2 text-center align-middle text-muted-foreground',
          $props.class,
        )
      "
    >
      <slot />
    </td>
  </tr>
</template>
