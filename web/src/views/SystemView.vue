<script setup lang="ts">
import { useSystemStatus } from "../composables/useSystemStatus";

const { system, loading, error } = useSystemStatus();
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
    <div v-else-if="loading || !system" class="loading-line">
      正在读取系统状态...
    </div>
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
