<script setup lang="ts">
import { Button } from "@/components/ui/button";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";

defineProps<{
  title: string;
  description: string;
  primaryLabel?: string;
  secondaryLabel?: string;
}>();

defineEmits<{
  primary: [];
  secondary: [];
}>();
</script>

<template>
  <Empty class="min-h-72 border border-dashed border-border bg-card">
    <EmptyHeader>
      <EmptyMedia v-if="$slots.icon" variant="icon">
        <slot name="icon" />
      </EmptyMedia>
      <EmptyTitle>{{ title }}</EmptyTitle>
      <EmptyDescription>{{ description }}</EmptyDescription>
    </EmptyHeader>
    <EmptyContent v-if="primaryLabel || secondaryLabel">
      <div class="flex flex-wrap justify-center gap-2">
        <Button
          v-if="primaryLabel"
          type="button"
          @click="$emit('primary')"
        >
          {{ primaryLabel }}
        </Button>
        <Button
          v-if="secondaryLabel"
          type="button"
          variant="outline"
          @click="$emit('secondary')"
        >
          {{ secondaryLabel }}
        </Button>
      </div>
    </EmptyContent>
  </Empty>
</template>
