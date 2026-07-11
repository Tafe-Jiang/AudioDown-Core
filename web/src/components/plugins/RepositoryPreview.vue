<script setup lang="ts">
import { CheckCircle2, LockKeyhole, TriangleAlert } from "@lucide/vue";

import type {
  RepositoryPluginPreview,
  RepositoryPreview,
} from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

const props = defineProps<{
  preview: RepositoryPreview;
  selectedPluginId: string;
}>();

defineEmits<{
  "update:selectedPluginId": [pluginId: string];
}>();

function selected(plugin: RepositoryPluginPreview) {
  return plugin.pluginId === props.selectedPluginId;
}
</script>

<template>
  <div class="grid gap-4">
    <div class="grid gap-1 border-b border-border pb-3">
      <div class="flex flex-wrap items-center gap-2">
        <h3 class="text-sm font-semibold">{{ preview.repository.name }}</h3>
        <Badge variant="outline">
          <LockKeyhole aria-hidden="true" />
          {{ preview.repository.commitSha.slice(0, 7) }}
        </Badge>
      </div>
      <p class="break-all text-xs text-muted-foreground">
        {{ preview.repository.sourceUrl }}
      </p>
    </div>

    <div class="grid gap-2" aria-label="仓库插件">
      <Button
        v-for="plugin in preview.plugins"
        :key="plugin.pluginId"
        type="button"
        variant="outline"
        class="h-auto w-full justify-start px-3 py-3 text-left"
        :aria-pressed="selected(plugin)"
        :data-selected="selected(plugin)"
        @click="$emit('update:selectedPluginId', plugin.pluginId)"
      >
        <CheckCircle2
          v-if="selected(plugin)"
          class="text-primary"
          aria-hidden="true"
        />
        <TriangleAlert
          v-else-if="plugin.requiresLifecycleScriptGrant"
          class="text-status-warning"
          aria-hidden="true"
        />
        <span v-else class="size-4" aria-hidden="true" />
        <span class="grid min-w-0 flex-1 gap-1">
          <span class="truncate font-medium">{{ plugin.name }}</span>
          <span class="flex flex-wrap items-center gap-2 text-xs">
            <Badge variant="secondary">{{ plugin.pluginType }}</Badge>
            <span class="text-muted-foreground">{{ plugin.version }}</span>
            <span v-if="plugin.alreadyInstalled" class="text-muted-foreground">
              已安装
            </span>
          </span>
        </span>
      </Button>
    </div>
  </div>
</template>
