<script setup lang="ts">
import { computed } from "vue";
import { CircleCheck, TriangleAlert } from "@lucide/vue";
import { useRoute } from "vue-router";

import StatusBadge from "@/components/common/StatusBadge.vue";
import { Separator } from "@/components/ui/separator";
import { SidebarTrigger, useSidebar } from "@/components/ui/sidebar";
import { useSystemStatus } from "@/composables/useSystemStatus";

const route = useRoute();
const { isMobile, state } = useSidebar();
const { system, loading } = useSystemStatus();

const title = computed(() => String(route.meta.title ?? "AudioDown"));
const supervisorAvailable = computed(
  () => system.value?.supervisor.available === true,
);
const supervisorLabel = computed(() => {
  if (loading.value) {
    return "正在检查 Supervisor";
  }
  return supervisorAvailable.value ? "Supervisor 可用" : "Supervisor 不可用";
});
const triggerLabel = computed(() => {
  if (isMobile.value) {
    return "打开主导航";
  }
  return state.value === "expanded" ? "折叠侧栏" : "展开侧栏";
});
</script>

<template>
  <header
    data-slot="app-header"
    class="sticky top-0 z-20 flex h-(--app-header-height) shrink-0 items-center border-b border-border bg-card"
  >
    <div class="flex w-full min-w-0 items-center gap-2 px-3 md:px-4">
      <SidebarTrigger :label="triggerLabel" :aria-label="triggerLabel" />
      <Separator
        orientation="vertical"
        class="data-[orientation=vertical]:h-4"
      />
      <span class="hidden text-sm font-semibold md:inline">AudioDown</span>
      <span class="hidden text-muted-foreground md:inline">/</span>
      <h1 class="truncate text-sm font-medium text-foreground">
        {{ title }}
      </h1>

      <div class="ml-auto flex shrink-0 items-center">
        <span
          class="grid size-7 place-items-center md:hidden"
          role="status"
          :aria-label="supervisorLabel"
        >
          <component
            :is="supervisorAvailable ? CircleCheck : TriangleAlert"
            class="size-4"
            :class="
              supervisorAvailable
                ? 'text-status-success'
                : 'text-status-warning'
            "
            aria-hidden="true"
          />
        </span>
        <div class="hidden items-center gap-2 md:flex">
          <StatusBadge tone="success" label="Core 在线" />
          <StatusBadge
            :tone="supervisorAvailable ? 'success' : 'warning'"
            :label="supervisorLabel"
          />
        </div>
      </div>
    </div>
  </header>
</template>
