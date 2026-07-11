<script setup lang="ts">
import { TriangleAlert } from "@lucide/vue";

import PageHeader from "../components/common/PageHeader.vue";
import StatusBadge from "../components/common/StatusBadge.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "../components/ui/alert";
import { Skeleton } from "../components/ui/skeleton";
import { useSystemStatus } from "../composables/useSystemStatus";

const { system, loading, error } = useSystemStatus();
const skeletonLabels = [
  "Core 版本",
  "Supervisor",
  "已安装插件",
  "开发者模式",
];
</script>

<template>
  <section class="grid gap-5">
    <PageHeader
      title="系统"
      description="查看 Core 与 Supervisor 的当前运行状态。"
    />

    <Alert v-if="error && !system" variant="destructive">
      <TriangleAlert aria-hidden="true" />
      <AlertTitle>系统状态读取失败</AlertTitle>
      <AlertDescription>{{ error }}</AlertDescription>
    </Alert>

    <dl
      v-if="loading || !system"
      data-system-status
      data-system-skeleton
      class="divide-y divide-border border-y border-border"
      aria-busy="true"
    >
      <div
        v-for="label in skeletonLabels"
        :key="label"
        data-system-row
        class="grid min-h-16 grid-cols-[minmax(8rem,1fr)_minmax(8rem,1.4fr)] items-center gap-4 py-3"
      >
        <dt class="text-sm text-muted-foreground">{{ label }}</dt>
        <dd><Skeleton class="h-5 w-32" /></dd>
      </div>
    </dl>

    <template v-else>
      <dl
        data-system-status
        class="divide-y divide-border border-y border-border"
      >
        <div
          data-system-row
          class="grid min-h-16 grid-cols-[minmax(8rem,1fr)_minmax(8rem,1.4fr)] items-center gap-4 py-3"
        >
          <dt class="text-sm text-muted-foreground">Core 版本</dt>
          <dd class="font-mono text-sm">{{ system.version }}</dd>
        </div>
        <div
          data-system-row
          class="grid min-h-16 grid-cols-[minmax(8rem,1fr)_minmax(8rem,1.4fr)] items-center gap-4 py-3"
        >
          <dt class="text-sm text-muted-foreground">Supervisor</dt>
          <dd>
            <StatusBadge
              :tone="system.supervisor.available ? 'success' : 'warning'"
              :label="
                system.supervisor.available ? '可用' : '不可用'
              "
            />
          </dd>
        </div>
        <div
          data-system-row
          class="grid min-h-16 grid-cols-[minmax(8rem,1fr)_minmax(8rem,1.4fr)] items-center gap-4 py-3"
        >
          <dt class="text-sm text-muted-foreground">已安装插件</dt>
          <dd class="text-sm font-medium tabular-nums">
            {{ system.pluginCount }}
          </dd>
        </div>
        <div
          data-system-row
          class="grid min-h-16 grid-cols-[minmax(8rem,1fr)_minmax(8rem,1.4fr)] items-center gap-4 py-3"
        >
          <dt class="text-sm text-muted-foreground">开发者模式</dt>
          <dd>
            <StatusBadge
              :tone="system.developmentMode ? 'warning' : 'neutral'"
              :label="system.developmentMode ? '已启用' : '未启用'"
            />
          </dd>
        </div>
      </dl>

      <Alert
        v-if="
          !system.supervisor.available || system.developmentMode
        "
      >
        <TriangleAlert class="text-status-warning" aria-hidden="true" />
        <AlertTitle>需要关注</AlertTitle>
        <AlertDescription class="grid gap-1">
          <p v-if="!system.supervisor.available">
            Supervisor 当前不可用。插件运行控制会保持禁用，请检查统一日志。
          </p>
          <p v-if="system.developmentMode">
            开发者模式已启用。安装脚本仍需逐次明确授权。
          </p>
        </AlertDescription>
      </Alert>
    </template>
  </section>
</template>
