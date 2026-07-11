<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { Plus } from "@lucide/vue";

import { api, type PluginItem, type PluginListResponse } from "../api/client";
import AsyncState from "../components/common/AsyncState.vue";
import EmptyState from "../components/common/EmptyState.vue";
import PageHeader from "../components/common/PageHeader.vue";
import StatusBadge from "../components/common/StatusBadge.vue";
import PluginRepositoryDialog from "../components/plugins/PluginRepositoryDialog.vue";
import { Button } from "../components/ui/button";
import { useSystemStatus } from "../composables/useSystemStatus";

const plugins = ref<PluginListResponse | null>(null);
const pluginError = ref("");
const repositoryOpen = ref(false);
const {
  system,
  loading: systemLoading,
  error: systemError,
  refresh: refreshSystem,
} = useSystemStatus();
const error = computed(() => pluginError.value || systemError.value || "");
const loading = computed(() => systemLoading.value || !plugins.value);

async function loadPlugins() {
  pluginError.value = "";
  try {
    plugins.value = await api.plugins();
  } catch {
    pluginError.value = "无法读取插件状态";
  }
}

async function handleInstalled(_plugin: PluginItem) {
  await Promise.all([loadPlugins(), refreshSystem()]);
}

onMounted(loadPlugins);
</script>

<template>
  <section class="grid gap-5">
    <PageHeader
      title="插件"
      description="检查公开仓库并管理安装到 Core 的插件。"
    >
      <template #actions>
        <StatusBadge
          v-if="system"
          :tone="system.supervisor.available ? 'success' : 'warning'"
          :label="
            system.supervisor.available
              ? 'Supervisor 可用'
              : 'Supervisor 不可用'
          "
        />
        <Button type="button" @click="repositoryOpen = true">
          <Plus aria-hidden="true" />
          添加仓库
        </Button>
      </template>
    </PageHeader>

    <AsyncState
      :loading="loading"
      :error="error"
      :empty="plugins?.items.length === 0"
      @retry="loadPlugins"
    >
      <template #empty>
        <EmptyState
          title="尚无已安装插件"
          description="当前核心不会内置任何内容来源。"
          primary-label="添加仓库"
          @primary="repositoryOpen = true"
        />
      </template>

      <div v-if="plugins" class="grid gap-2">
        <div
          v-for="plugin in plugins.items"
          :key="plugin.pluginId"
          class="flex items-center justify-between gap-3 border-b border-border py-3"
        >
          <span class="min-w-0">
            <strong class="block truncate text-sm">{{ plugin.name }}</strong>
            <small class="text-muted-foreground">
              {{ plugin.pluginType }} · {{ plugin.version }}
            </small>
          </span>
          <StatusBadge tone="neutral" :label="plugin.status" />
        </div>
      </div>
    </AsyncState>

    <PluginRepositoryDialog
      v-model:open="repositoryOpen"
      :supervisor-available="system?.supervisor.available ?? false"
      :development-mode="system?.developmentMode ?? false"
      @installed="handleInstalled"
    />
  </section>
</template>
