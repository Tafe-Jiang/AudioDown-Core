<script setup lang="ts">
import { onBeforeUnmount, ref } from "vue";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import {
  Drawer,
  DrawerContent,
  DrawerDescription,
  DrawerFooter,
  DrawerHeader,
  DrawerTitle,
} from "@/components/ui/drawer";

export interface ResponsiveDialogProps {
  open: boolean;
  title: string;
  description?: string;
}

defineProps<ResponsiveDialogProps>();
const emit = defineEmits<{
  "update:open": [open: boolean];
  close: [];
}>();

const mediaQuery =
  typeof window === "undefined" || typeof window.matchMedia !== "function"
    ? undefined
    : window.matchMedia("(max-width: 760px)");
const mobile = ref(mediaQuery?.matches ?? false);

function updateMobile(event: MediaQueryListEvent) {
  mobile.value = event.matches;
}

mediaQuery?.addEventListener("change", updateMobile);
onBeforeUnmount(() => {
  mediaQuery?.removeEventListener("change", updateMobile);
});

function updateOpen(open: boolean) {
  emit("update:open", open);
  if (!open) {
    emit("close");
  }
}
</script>

<template>
  <Drawer
    v-if="mobile"
    :open="open"
    @update:open="updateOpen"
  >
    <DrawerContent>
      <DrawerHeader>
        <DrawerTitle>{{ title }}</DrawerTitle>
        <DrawerDescription v-if="description">
          {{ description }}
        </DrawerDescription>
      </DrawerHeader>
      <div class="min-h-0 overflow-y-auto px-4 pb-4">
        <slot />
      </div>
      <DrawerFooter v-if="$slots.footer">
        <slot name="footer" />
      </DrawerFooter>
    </DrawerContent>
  </Drawer>

  <Dialog
    v-else
    :open="open"
    @update:open="updateOpen"
  >
    <DialogContent>
      <DialogHeader>
        <DialogTitle>{{ title }}</DialogTitle>
        <DialogDescription v-if="description">
          {{ description }}
        </DialogDescription>
      </DialogHeader>
      <div class="min-h-0 overflow-y-auto">
        <slot />
      </div>
      <DialogFooter v-if="$slots.footer">
        <slot name="footer" />
      </DialogFooter>
    </DialogContent>
  </Dialog>
</template>
