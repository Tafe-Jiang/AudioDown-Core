<script setup lang="ts">
const navigation = [
  { to: "/discover", label: "发现", marker: "D" },
  { to: "/search", label: "搜索", marker: "S" },
  { to: "/plugins", label: "插件", marker: "P" },
  { to: "/logs", label: "日志", marker: "L" },
  { to: "/system", label: "系统", marker: "I" },
];
</script>

<template>
  <div class="app-shell">
    <aside class="sidebar">
      <div class="brand">
        <span class="brand-mark" aria-hidden="true">
          <i></i><i></i><i></i><i></i>
        </span>
        <span>
          <strong>AudioDown</strong>
          <small>Core 1.0</small>
        </span>
      </div>

      <nav class="primary-nav" aria-label="主导航">
        <RouterLink
          v-for="item in navigation"
          :key="item.to"
          :to="item.to"
        >
          <span class="nav-marker" aria-hidden="true">{{ item.marker }}</span>
          {{ item.label }}
        </RouterLink>
      </nav>

      <div class="sidebar-foot">
        <span class="status-dot"></span>
        空核心模式
      </div>
    </aside>

    <main class="main-content">
      <RouterView />
    </main>
  </div>
</template>
