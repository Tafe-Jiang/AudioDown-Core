<script setup lang="ts">
import { ref } from "vue";

import AppHeader from "./AppHeader.vue";
import AppSidebar from "./AppSidebar.vue";
import {
  SidebarInset,
  SidebarProvider,
} from "@/components/ui/sidebar";
import { useSystemStatus } from "@/composables/useSystemStatus";

const storageKey = "audiodown.sidebar.collapsed";
const initiallyCollapsed =
  typeof localStorage !== "undefined" &&
  localStorage.getItem(storageKey) === "true";
const sidebarOpen = ref(!initiallyCollapsed);

useSystemStatus();

function updateSidebar(open: boolean) {
  sidebarOpen.value = open;
  localStorage.setItem(storageKey, String(!open));
}
</script>

<template>
  <SidebarProvider
    :open="sidebarOpen"
    sidebar-width="var(--app-sidebar-width)"
    sidebar-icon-width="var(--app-sidebar-icon-width)"
    @update:open="updateSidebar"
  >
    <AppSidebar />
    <SidebarInset>
      <AppHeader />
      <main
        id="main-content"
        class="mx-auto w-full max-w-[1440px] flex-1 px-4 py-5 md:px-6 md:py-6"
        tabindex="-1"
      >
        <slot />
      </main>
    </SidebarInset>
  </SidebarProvider>
</template>
