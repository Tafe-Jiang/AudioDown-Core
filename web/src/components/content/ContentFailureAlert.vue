<script setup lang="ts">
import { TriangleAlert } from "@lucide/vue";

import type { ContentFailure } from "@/api/client";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";

defineProps<{
  failures: ContentFailure[];
}>();
</script>

<template>
  <Alert v-if="failures.length > 0">
    <TriangleAlert aria-hidden="true" />
    <AlertTitle>部分来源暂不可用</AlertTitle>
    <AlertDescription>
      <ul class="grid gap-1.5">
        <li
          v-for="failure in failures"
          :key="`${failure.source.platformId}:${failure.source.pluginId}:${failure.code}`"
          class="break-words"
        >
          {{ failure.summary }}
          <span class="block break-all text-xs text-muted-foreground">
            {{ failure.source.platformId }} ·
            {{ failure.source.pluginId }} ·
            {{ failure.code }}
          </span>
        </li>
      </ul>
    </AlertDescription>
  </Alert>
</template>
