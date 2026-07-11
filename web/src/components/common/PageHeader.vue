<script setup lang="ts">
defineProps<{
  title: string;
  description?: string;
  eyebrow?: string;
}>();
</script>

<template>
  <header
    class="flex min-h-13 flex-wrap items-start justify-between gap-3 border-b border-border pb-3"
  >
    <div class="min-w-0">
      <p
        v-if="eyebrow"
        class="mb-1 text-xs font-medium text-muted-foreground"
      >
        {{ eyebrow }}
      </p>
      <h1 class="text-xl font-semibold leading-7 text-foreground">
        {{ title }}
      </h1>
      <p
        v-if="description"
        class="mt-1 max-w-3xl text-sm leading-5 text-muted-foreground"
      >
        {{ description }}
      </p>
    </div>
    <div v-if="$slots.actions" class="flex shrink-0 items-center gap-2">
      <slot name="actions" />
    </div>
  </header>
</template>
