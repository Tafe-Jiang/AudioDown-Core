<script setup lang="ts">
import { onMounted, ref } from "vue";

import {
  api,
  type PluginListResponse,
  type SystemResponse,
} from "../api/client";

const system = ref<SystemResponse | null>(null);
const plugins = ref<PluginListResponse | null>(null);
const error = ref("");

onMounted(async () => {
  try {
    [system.value, plugins.value] = await Promise.all([
      api.system(),
      api.plugins(),
    ]);
  } catch {
    error.value = "无法读取插件状态";
  }
});
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <p class="eyebrow">RUNTIME</p>
        <h1>插件</h1>
      </div>
      <span
        v-if="system"
        class="availability"
        :class="{ online: system.supervisor.available }"
      >
        <span></span>
        Supervisor {{ system.supervisor.available ? "可用" : "不可用" }}
      </span>
    </header>

    <div v-if="error" class="notice error-notice">{{ error }}</div>
    <div v-else-if="!system || !plugins" class="loading-line">
      正在读取插件运行状态...
    </div>
    <template v-else>
      <div class="summary-strip">
        <div>
          <span>已登记插件</span>
          <strong>{{ system.pluginCount }}</strong>
        </div>
        <div>
          <span>管理服务</span>
          <strong>{{
            system.supervisor.available ? "ONLINE" : "OFFLINE"
          }}</strong>
        </div>
      </div>

      <div v-if="plugins.items.length === 0" class="empty-list">
        <p class="empty-code">NO_PLUGINS</p>
        <h2>尚无已安装插件</h2>
        <p>当前核心不会内置任何内容来源。</p>
      </div>

      <div v-else class="data-table">
        <div class="table-row table-head">
          <span>名称</span><span>版本</span><span>状态</span>
        </div>
        <div v-for="plugin in plugins.items" :key="plugin.pluginId" class="table-row">
          <strong>{{ plugin.name }}</strong>
          <span>{{ plugin.version }}</span>
          <span>{{ plugin.status }}</span>
        </div>
      </div>
    </template>
  </section>
</template>
