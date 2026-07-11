<script setup lang="ts">
import { AudioLines, CircleCheck, TriangleAlert } from "@lucide/vue";
import { useRoute } from "vue-router";

import { navigation } from "./navigation";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarRail,
  useSidebar,
} from "@/components/ui/sidebar";
import { useSystemStatus } from "@/composables/useSystemStatus";

const route = useRoute();
const { setOpenMobile } = useSidebar();
const { system, loading } = useSystemStatus();
</script>

<template>
  <Sidebar collapsible="icon">
    <SidebarHeader class="border-b border-sidebar-border p-3">
      <div class="flex h-9 items-center gap-2 overflow-hidden px-1">
        <span
          class="grid size-8 shrink-0 place-items-center rounded-md bg-sidebar-primary text-sidebar-primary-foreground"
          aria-hidden="true"
        >
          <AudioLines class="size-4" />
        </span>
        <span
          class="min-w-0 group-data-[collapsible=icon]:hidden"
        >
          <strong class="block truncate text-sm font-semibold text-white">
            AudioDown
          </strong>
          <small class="block truncate text-xs text-sidebar-foreground/65">
            Core 1.0
          </small>
        </span>
      </div>
    </SidebarHeader>

    <SidebarContent class="p-2">
      <nav aria-label="主导航">
        <SidebarMenu>
          <SidebarMenuItem v-for="item in navigation" :key="item.to">
            <SidebarMenuButton
              as-child
              :tooltip="item.label"
              :data-active="route.path === item.to"
            >
              <RouterLink
                :to="item.to"
                :aria-label="item.label"
                :aria-current="route.path === item.to ? 'page' : undefined"
                :data-navigation-link="item.to"
                @click="setOpenMobile(false)"
              >
                <component :is="item.icon" aria-hidden="true" />
                <span>{{ item.label }}</span>
              </RouterLink>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </nav>
    </SidebarContent>

    <SidebarFooter class="border-t border-sidebar-border p-3">
      <div
        class="flex h-10 items-center gap-2 overflow-hidden px-1 text-xs text-sidebar-foreground/75"
      >
        <component
          :is="system?.supervisor.available ? CircleCheck : TriangleAlert"
          class="size-4 shrink-0"
          :class="
            system?.supervisor.available
              ? 'text-status-success'
              : 'text-status-warning'
          "
          aria-hidden="true"
        />
        <span class="truncate group-data-[collapsible=icon]:hidden">
          {{
            loading
              ? "正在检查 Supervisor"
              : system?.supervisor.available
                ? "空核心 · Supervisor 可用"
                : "空核心 · Supervisor 不可用"
          }}
        </span>
      </div>
    </SidebarFooter>

    <SidebarRail />
  </Sidebar>
</template>
