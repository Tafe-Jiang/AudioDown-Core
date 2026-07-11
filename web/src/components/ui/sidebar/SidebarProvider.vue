<script setup lang="ts">
import type { HTMLAttributes, Ref } from "vue";
import {
  useEventListener,
  useMediaQuery,
  useVModel,
} from "@vueuse/core";
import { TooltipProvider } from "reka-ui";
import { computed, ref } from "vue";

import { cn } from "@/lib/utils";
import {
  provideSidebarContext,
  SIDEBAR_KEYBOARD_SHORTCUT,
} from "./utils";

const props = withDefaults(defineProps<{
  defaultOpen?: boolean;
  open?: boolean;
  class?: HTMLAttributes["class"];
  sidebarWidth?: string;
  sidebarIconWidth?: string;
}>(), {
  defaultOpen: true,
  open: undefined,
  sidebarWidth: "14.5rem",
  sidebarIconWidth: "4rem",
});

const emits = defineEmits<{
  "update:open": [open: boolean];
}>();

const isMobile = useMediaQuery("(max-width: 767px)");
const openMobile = ref(false);

const open = useVModel(props, "open", emits, {
  defaultValue: props.defaultOpen ?? false,
  passive: (props.open === undefined) as false,
}) as Ref<boolean>;

function setOpen(value: boolean) {
  open.value = value;
}

function setOpenMobile(value: boolean) {
  openMobile.value = value;
}

function toggleSidebar() {
  return isMobile.value
    ? setOpenMobile(!openMobile.value)
    : setOpen(!open.value);
}

useEventListener("keydown", (event: KeyboardEvent) => {
  if (event.key === SIDEBAR_KEYBOARD_SHORTCUT && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    toggleSidebar();
  }
});

const state = computed(() => open.value ? "expanded" : "collapsed");

provideSidebarContext({
  state,
  open,
  setOpen,
  isMobile,
  openMobile,
  setOpenMobile,
  toggleSidebar,
});
</script>

<template>
  <TooltipProvider :delay-duration="0">
    <div
      data-slot="sidebar-wrapper"
      :style="{
        '--sidebar-width': sidebarWidth,
        '--sidebar-width-icon': sidebarIconWidth,
      }"
      :class="cn('group/sidebar-wrapper has-data-[variant=inset]:bg-sidebar flex min-h-svh w-full', props.class)"
      v-bind="$attrs"
    >
      <slot />
    </div>
  </TooltipProvider>
</template>
