<script setup lang="ts">
import { computed } from "vue";
import {
  CircleCheckIcon,
  InfoIcon,
  OctagonXIcon,
  TriangleAlertIcon,
} from "@lucide/vue";

import { Badge } from "@/components/ui/badge";

export type StatusTone = "neutral" | "success" | "warning" | "danger";

const props = withDefaults(
  defineProps<{
    tone?: StatusTone;
    label: string;
  }>(),
  {
    tone: "neutral",
  },
);

const icon = computed(() => {
  switch (props.tone) {
    case "success":
      return CircleCheckIcon;
    case "warning":
      return TriangleAlertIcon;
    case "danger":
      return OctagonXIcon;
    default:
      return InfoIcon;
  }
});

const toneClass = computed(() => {
  switch (props.tone) {
    case "success":
      return "border-status-success/25 bg-status-success/10 text-status-success";
    case "warning":
      return "border-status-warning/25 bg-status-warning/10 text-status-warning";
    case "danger":
      return "border-status-danger/25 bg-status-danger/10 text-status-danger";
    default:
      return "border-border bg-muted text-muted-foreground";
  }
});
</script>

<template>
  <Badge
    as="span"
    variant="outline"
    role="status"
    :data-tone="tone"
    :class="toneClass"
  >
    <component :is="icon" aria-hidden="true" />
    {{ label }}
  </Badge>
</template>
