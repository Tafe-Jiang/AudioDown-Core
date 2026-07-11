<script setup lang="ts">
import type { CSSProperties } from "vue";
import { computed } from "vue";
import {
  CircleCheckIcon,
  InfoIcon,
  Loader2Icon,
  OctagonXIcon,
  TriangleAlertIcon,
  XIcon,
} from "@lucide/vue";
import type { ToasterProps } from "vue-sonner";
import { Toaster as Sonner } from "vue-sonner";

import { cn } from "@/lib/utils";

const props = defineProps<ToasterProps>();
const forwardedProps = computed(() => {
  const {
    class: _class,
    style: _style,
    toastOptions: _toastOptions,
    ...forwarded
  } = props;
  return forwarded;
});
const toastOptions = computed(() => ({
  ...props.toastOptions,
  classes: {
    ...props.toastOptions?.classes,
    toast: cn("rounded-md", props.toastOptions?.classes?.toast),
  },
}));
const sonnerStyle = computed(
  () =>
    ({
      ...props.style,
      "--normal-bg": "var(--popover)",
      "--normal-text": "var(--popover-foreground)",
      "--normal-border": "var(--border)",
      "--border-radius": "var(--radius)",
      "--gray2": "color-mix(in srgb, var(--popover) 90%, transparent)",
      "--gray3": "var(--border)",
      "--gray4": "var(--border)",
      "--gray5": "var(--border)",
      "--gray12": "var(--popover-foreground)",
    }) as CSSProperties,
);
</script>

<template>
  <Sonner
    v-bind="forwardedProps"
    :class="cn('toaster group', props.class)"
    :style="sonnerStyle"
    :toast-options="toastOptions"
  >
    <template #success-icon>
      <CircleCheckIcon class="size-4" />
    </template>
    <template #info-icon>
      <InfoIcon class="size-4" />
    </template>
    <template #warning-icon>
      <TriangleAlertIcon class="size-4" />
    </template>
    <template #error-icon>
      <OctagonXIcon class="size-4" />
    </template>
    <template #loading-icon>
      <div>
        <Loader2Icon class="size-4 animate-spin" />
      </div>
    </template>
    <template #close-icon>
      <XIcon class="size-4" />
    </template>
  </Sonner>
</template>
