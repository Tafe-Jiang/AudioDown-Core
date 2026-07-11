<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { TriangleAlert } from "@lucide/vue";

import type {
  PluginItem,
  PluginRunMode,
  PluginSettings,
} from "@/api/client";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Field,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";

const props = defineProps<{
  open: boolean;
  plugin: PluginItem | null;
  busy: boolean;
  error: string;
}>();

const emit = defineEmits<{
  "update:open": [open: boolean];
  save: [settings: PluginSettings];
}>();

const runMode = ref<PluginRunMode>("on_demand");
const priority = ref(100);
const formId = computed(
  () => `plugin-settings-${props.plugin?.pluginId ?? "none"}`,
);

function resetForm() {
  if (!props.plugin) {
    return;
  }
  runMode.value = props.plugin.runMode;
  priority.value = props.plugin.priority;
}

function save() {
  if (!props.plugin || priority.value < 0 || priority.value > 1000) {
    return;
  }
  emit("save", {
    enabled: props.plugin.enabled,
    runMode: runMode.value,
    priority: Number(priority.value),
  });
}

watch(() => props.plugin, resetForm, { immediate: true });
watch(
  () => props.open,
  (open) => {
    if (open) {
      resetForm();
    }
  },
);
</script>

<template>
  <Sheet :open="open" @update:open="emit('update:open', $event)">
    <SheetContent class="w-full sm:max-w-sm">
      <SheetHeader>
        <SheetTitle>设置 {{ plugin?.name }}</SheetTitle>
        <SheetDescription>
          调整运行模式和聚合优先级。
        </SheetDescription>
      </SheetHeader>

      <form
        :id="formId"
        class="grid gap-4 px-4"
        @submit.prevent="save"
      >
        <Field>
          <FieldLabel for="plugin-run-mode">运行模式</FieldLabel>
          <Select
            v-model="runMode"
            name="run-mode"
            :disabled="busy"
          >
            <SelectTrigger id="plugin-run-mode" class="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="on_demand">按需运行</SelectItem>
              <SelectItem value="always">持续运行</SelectItem>
            </SelectContent>
          </Select>
          <FieldDescription>
            按需模式仅在调用时启动，持续模式由 Supervisor 保持运行。
          </FieldDescription>
        </Field>

        <Field>
          <FieldLabel for="plugin-priority">优先级</FieldLabel>
          <Input
            id="plugin-priority"
            v-model="priority"
            name="priority"
            type="number"
            min="0"
            max="1000"
            step="1"
            :disabled="busy"
          />
          <FieldDescription>数值范围 0 至 1000。</FieldDescription>
        </Field>

        <Alert v-if="error" variant="destructive">
          <TriangleAlert aria-hidden="true" />
          <AlertTitle>保存失败</AlertTitle>
          <AlertDescription>{{ error }}</AlertDescription>
        </Alert>
      </form>

      <SheetFooter>
        <Button
          type="button"
          variant="outline"
          :disabled="busy"
          @click="emit('update:open', false)"
        >
          取消
        </Button>
        <Button type="submit" :form="formId" :disabled="busy">
          {{ busy ? "保存中" : "保存" }}
        </Button>
      </SheetFooter>
    </SheetContent>
  </Sheet>
</template>
