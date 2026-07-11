<script setup lang="ts">
import { onMounted, ref } from "vue";

import { api, type SystemResponse } from "../api/client";

const system = ref<SystemResponse | null>(null);
const error = ref("");

onMounted(async () => {
  try {
    system.value = await api.system();
  } catch {
    error.value = "无法读取系统状态";
  }
});
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <p class="eyebrow">CORE</p>
        <h1>系统</h1>
      </div>
    </header>

    <div v-if="error" class="notice error-notice">{{ error }}</div>
    <div v-else-if="!system" class="loading-line">正在读取系统状态...</div>
    <dl v-else class="system-list">
      <div>
        <dt>Core 版本</dt>
        <dd>{{ system.version }}</dd>
      </div>
      <div>
        <dt>Supervisor</dt>
        <dd>{{ system.supervisor.available ? "可用" : "不可用" }}</dd>
      </div>
      <div>
        <dt>插件记录</dt>
        <dd>{{ system.pluginCount }}</dd>
      </div>
    </dl>
  </section>
</template>
