<script setup lang="ts">
import { onMounted, ref } from "vue";

import { api, type EmptyStateResponse } from "../api/client";

const state = ref<EmptyStateResponse | null>(null);
const error = ref("");

onMounted(async () => {
  try {
    state.value = await api.discover();
  } catch {
    error.value = "无法读取发现状态";
  }
});
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <p class="eyebrow">CONTENT</p>
        <h1>发现</h1>
      </div>
      <span class="mode-badge">空状态</span>
    </header>

    <div v-if="error" class="notice error-notice">{{ error }}</div>
    <div v-else-if="!state" class="loading-line">正在读取 Core 状态...</div>
    <div v-else class="empty-state">
      <div class="empty-signal" aria-hidden="true">
        <span></span><span></span><span></span><span></span><span></span>
      </div>
      <p class="empty-code">{{ state.reason }}</p>
      <h2>{{ state.title }}</h2>
      <p>安装内容插件后，发现频道会显示在这里。</p>
      <RouterLink class="primary-action" to="/plugins">
        {{ state.actionLabel }}
      </RouterLink>
    </div>
  </section>
</template>
