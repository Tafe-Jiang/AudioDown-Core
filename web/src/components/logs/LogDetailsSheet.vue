<script setup lang="ts">
import type { StructuredLog } from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";

defineProps<{
  open: boolean;
  log: StructuredLog | null;
}>();

const emit = defineEmits<{
  "update:open": [open: boolean];
}>();
</script>

<template>
  <Sheet :open="open" @update:open="emit('update:open', $event)">
    <SheetContent class="w-full sm:max-w-md">
      <SheetHeader>
        <SheetTitle>日志详情</SheetTitle>
        <SheetDescription>
          Core 返回的结构化日志字段。
        </SheetDescription>
      </SheetHeader>

      <ScrollArea v-if="log" class="min-h-0 flex-1 px-4">
        <dl data-log-details class="grid gap-4 pb-6 text-sm">
          <div class="grid gap-1">
            <dt class="text-xs text-muted-foreground">ID</dt>
            <dd class="break-all font-mono">{{ log.id }}</dd>
          </div>
          <div class="grid gap-1">
            <dt class="text-xs text-muted-foreground">时间</dt>
            <dd class="break-all">
              <time :datetime="log.timestamp">{{ log.timestamp }}</time>
            </dd>
          </div>
          <div class="grid gap-1">
            <dt class="text-xs text-muted-foreground">级别</dt>
            <dd><Badge variant="outline">{{ log.level }}</Badge></dd>
          </div>
          <div class="grid gap-1">
            <dt class="text-xs text-muted-foreground">组件</dt>
            <dd class="break-words">{{ log.component }}</dd>
          </div>
          <div class="grid gap-1">
            <dt class="text-xs text-muted-foreground">消息</dt>
            <dd class="whitespace-pre-wrap break-words">{{ log.message }}</dd>
          </div>
          <div v-if="log.pluginId" class="grid gap-1">
            <dt class="text-xs text-muted-foreground">插件 ID</dt>
            <dd class="break-all font-mono">{{ log.pluginId }}</dd>
          </div>
        </dl>
      </ScrollArea>
    </SheetContent>
  </Sheet>
</template>
