<script setup lang="ts">
import { onMounted, ref } from "vue";

import { api, type LogListResponse } from "../api/client";

const logs = ref<LogListResponse | null>(null);
const error = ref("");

onMounted(async () => {
  try {
    logs.value = await api.logs();
  } catch {
    error.value = "无法读取日志";
  }
});
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <p class="eyebrow">OBSERVABILITY</p>
        <h1>日志</h1>
      </div>
    </header>

    <div v-if="error" class="notice error-notice">{{ error }}</div>
    <div v-else-if="!logs" class="loading-line">正在读取日志...</div>
    <div v-else-if="logs.items.length === 0" class="empty-list">
      <p class="empty-code">NO_LOG_ENTRIES</p>
      <h2>暂无结构化日志</h2>
      <p>Core、Supervisor 和插件事件会按时间显示在这里。</p>
    </div>
    <div v-else class="data-table log-table">
      <div class="table-row table-head">
        <span>时间</span><span>组件</span><span>消息</span>
      </div>
      <div v-for="log in logs.items" :key="log.id" class="table-row">
        <time>{{ log.timestamp }}</time>
        <span>{{ log.component }}</span>
        <strong>{{ log.message }}</strong>
      </div>
    </div>
  </section>
</template>
