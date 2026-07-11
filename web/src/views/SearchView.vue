<script setup lang="ts">
import { onMounted, ref } from "vue";

import { api, type EmptyStateResponse } from "../api/client";

const query = ref("");
const state = ref<EmptyStateResponse | null>(null);
const error = ref("");
const loading = ref(false);

async function search() {
  loading.value = true;
  error.value = "";
  try {
    state.value = await api.search(query.value);
  } catch {
    error.value = "无法读取搜索状态";
  } finally {
    loading.value = false;
  }
}

onMounted(search);
</script>

<template>
  <section class="page">
    <header class="page-header">
      <div>
        <p class="eyebrow">QUERY</p>
        <h1>搜索</h1>
      </div>
    </header>

    <form class="search-bar" @submit.prevent="search">
      <label class="sr-only" for="search-query">搜索内容</label>
      <input
        id="search-query"
        v-model="query"
        type="search"
        placeholder="输入关键词"
      />
      <button type="submit" :disabled="loading">
        {{ loading ? "查询中" : "搜索" }}
      </button>
    </form>

    <div v-if="error" class="notice error-notice">{{ error }}</div>
    <div v-else-if="!state" class="loading-line">正在读取 Core 状态...</div>
    <div v-else class="empty-state compact-empty">
      <div class="empty-signal" aria-hidden="true">
        <span></span><span></span><span></span><span></span><span></span>
      </div>
      <p class="empty-code">{{ state.reason }}</p>
      <h2>{{ state.title }}</h2>
      <p>安装内容插件后即可执行聚合搜索。</p>
      <RouterLink class="primary-action" to="/plugins">
        {{ state.actionLabel }}
      </RouterLink>
    </div>
  </section>
</template>
