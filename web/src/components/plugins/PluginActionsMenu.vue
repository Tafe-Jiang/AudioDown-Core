<script setup lang="ts">
import {
  MoreHorizontal,
  Play,
  Settings2,
  Square,
  Trash2,
} from "@lucide/vue";

import type { PluginItem } from "@/api/client";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";

const props = withDefaults(
  defineProps<{
    plugin: PluginItem;
    busy: boolean;
    supervisorAvailable: boolean;
    context?: "desktop" | "mobile";
  }>(),
  {
    context: "desktop",
  },
);

const emit = defineEmits<{
  start: [];
  stop: [];
  settings: [triggerId: string];
  uninstall: [];
}>();

function triggerId() {
  return `${props.context}-plugin-actions-${props.plugin.pluginId}`;
}

function isRunning() {
  return ["running", "starting", "healthy"].includes(props.plugin.status);
}
</script>

<template>
  <div class="flex items-center justify-end gap-1">
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger as-child>
          <Button
            v-if="isRunning()"
            type="button"
            variant="ghost"
            size="icon-sm"
            :aria-label="`停止 ${plugin.name}`"
            :title="`停止 ${plugin.name}`"
            :disabled="busy || !supervisorAvailable"
            @click="emit('stop')"
          >
            <Square aria-hidden="true" />
          </Button>
          <Button
            v-else
            type="button"
            variant="ghost"
            size="icon-sm"
            :aria-label="`启动 ${plugin.name}`"
            :title="`启动 ${plugin.name}`"
            :disabled="busy || !supervisorAvailable || !plugin.enabled"
            @click="emit('start')"
          >
            <Play aria-hidden="true" />
          </Button>
        </TooltipTrigger>
        <TooltipContent>
          {{ isRunning() ? `停止 ${plugin.name}` : `启动 ${plugin.name}` }}
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>

    <DropdownMenu>
      <DropdownMenuTrigger as-child>
        <Button
          :id="triggerId()"
          type="button"
          variant="ghost"
          size="icon-sm"
          :aria-label="`${plugin.name} 更多操作`"
          :disabled="busy"
        >
          <MoreHorizontal aria-hidden="true" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem
          data-action="settings"
          :disabled="!supervisorAvailable"
          @select="emit('settings', triggerId())"
        >
          <Settings2 aria-hidden="true" />
          设置
        </DropdownMenuItem>
        <DropdownMenuSeparator />
        <DropdownMenuItem
          data-action="uninstall"
          variant="destructive"
          :disabled="!supervisorAvailable"
          @select="emit('uninstall')"
        >
          <Trash2 aria-hidden="true" />
          卸载
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  </div>
</template>
