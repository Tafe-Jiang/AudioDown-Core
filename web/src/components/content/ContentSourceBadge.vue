<script setup lang="ts">
import { Boxes } from "@lucide/vue";

import type { ContentSource } from "@/api/client";
import { Badge } from "@/components/ui/badge";

defineProps<{
  source: ContentSource;
}>();
</script>

<template>
  <div class="flex min-w-0 flex-wrap items-center gap-1.5">
    <Badge variant="secondary">
      <Boxes aria-hidden="true" />
      {{ source.platformId }}
    </Badge>
    <Badge variant="outline">
      {{ source.pluginName }} {{ source.pluginVersion }}
    </Badge>
    <span
      data-plugin-id
      class="min-w-0 break-all text-xs text-muted-foreground"
    >
      {{ source.pluginId }}
    </span>
  </div>
</template>
