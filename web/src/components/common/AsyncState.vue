<script setup lang="ts">
import { RotateCcwIcon, TriangleAlertIcon } from "@lucide/vue";

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";

export interface AsyncStateProps {
  loading: boolean;
  error?: string;
  empty?: boolean;
}

defineProps<AsyncStateProps>();
defineEmits<{
  retry: [];
}>();
</script>

<template>
  <div
    v-if="loading"
    class="grid min-h-40 gap-3"
    role="status"
    aria-label="加载中"
  >
    <Skeleton class="h-8 w-48" />
    <Skeleton class="h-12 w-full" />
    <Skeleton class="h-12 w-full" />
  </div>

  <Alert v-else-if="error" variant="destructive">
    <TriangleAlertIcon aria-hidden="true" />
    <AlertTitle>加载失败</AlertTitle>
    <AlertDescription>{{ error }}</AlertDescription>
    <Button
      class="mt-2 w-fit"
      type="button"
      variant="outline"
      size="sm"
      aria-label="重试"
      @click="$emit('retry')"
    >
      <RotateCcwIcon aria-hidden="true" />
      重试
    </Button>
  </Alert>

  <slot v-else-if="empty" name="empty" />
  <slot v-else />
</template>
