<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { TriangleAlert } from "@lucide/vue";
import { toast } from "vue-sonner";

import RepositoryPreview from "./RepositoryPreview.vue";
import type {
  PluginItem,
  RepositoryPreview as RepositoryPreviewData,
} from "@/api/client";
import { api } from "@/api/client";
import ResponsiveDialog from "@/components/common/ResponsiveDialog.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Field,
  FieldDescription,
  FieldLabel,
} from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Skeleton } from "@/components/ui/skeleton";

export type RepositoryDialogStep = "url" | "preview" | "installing";

const props = defineProps<{
  open: boolean;
  supervisorAvailable: boolean;
  developmentMode: boolean;
}>();

const emit = defineEmits<{
  "update:open": [open: boolean];
  installed: [plugin: PluginItem];
}>();

const formId = "plugin-repository-form";
const step = ref<RepositoryDialogStep>("url");
const repositoryUrl = ref("");
const preview = ref<RepositoryPreviewData | null>(null);
const selectedPluginId = ref("");
const inspecting = ref(false);
const error = ref("");
const riskApproved = ref(false);
const developerToken = ref("");
const scopeGrantDecisions = ref<Record<string, boolean>>({});

const selectedPlugin = computed(
  () =>
    preview.value?.plugins.find(
      (plugin) => plugin.pluginId === selectedPluginId.value,
    ) ?? null,
);
const requiresRiskGrant = computed(
  () => selectedPlugin.value?.requiresLifecycleScriptGrant === true,
);
const requiredScopeDecisionsComplete = computed(
  () =>
    selectedPlugin.value?.credentials.requiredScopes.every(
      (declaration) =>
        scopeGrantDecisions.value[`required:${declaration.scope}`] === true,
    ) ?? true,
);
const installDisabled = computed(() => {
  if (
    !props.supervisorAvailable ||
    !selectedPlugin.value ||
    selectedPlugin.value.alreadyInstalled
  ) {
    return true;
  }
  if (!requiredScopeDecisionsComplete.value) {
    return true;
  }
  if (!requiresRiskGrant.value) {
    return false;
  }
  return (
    !props.developmentMode ||
    !riskApproved.value ||
    developerToken.value.length === 0
  );
});

function clearDeveloperToken() {
  developerToken.value = "";
}

function resetRiskApproval() {
  riskApproved.value = false;
  clearDeveloperToken();
}

function resetScopeGrantDecisions() {
  scopeGrantDecisions.value = {};
}

function resetPluginReview() {
  resetRiskApproval();
  resetScopeGrantDecisions();
}

function resetDialog() {
  step.value = "url";
  repositoryUrl.value = "";
  preview.value = null;
  selectedPluginId.value = "";
  inspecting.value = false;
  error.value = "";
  resetPluginReview();
}

function closeDialog() {
  resetDialog();
  emit("update:open", false);
}

function handleOpen(open: boolean) {
  if (!open) {
    closeDialog();
  }
}

function goBack() {
  step.value = "url";
  preview.value = null;
  selectedPluginId.value = "";
  error.value = "";
  resetPluginReview();
}

async function inspectRepository() {
  const url = repositoryUrl.value.trim();
  if (!url) {
    error.value = "请输入公开仓库地址";
    return;
  }

  inspecting.value = true;
  error.value = "";
  try {
    const result = await api.inspectRepository(url);
    preview.value = result;
    selectedPluginId.value = result.plugins[0]?.pluginId ?? "";
    resetPluginReview();
    step.value = "preview";
  } catch {
    error.value = "仓库检查失败，请确认地址和仓库内容";
  } finally {
    inspecting.value = false;
  }
}

async function installPlugin() {
  const plugin = selectedPlugin.value;
  const repository = preview.value;
  if (!plugin || !repository || installDisabled.value) {
    return;
  }

  step.value = "installing";
  error.value = "";
  try {
    const installed = await api.installPlugin(
      repository.snapshotId,
      plugin.pluginId,
      requiresRiskGrant.value && riskApproved.value,
      requiresRiskGrant.value ? developerToken.value : undefined,
    );
    clearDeveloperToken();
    toast.success("插件安装完成");
    emit("installed", installed);
    closeDialog();
  } catch {
    clearDeveloperToken();
    step.value = "preview";
    error.value = "安装失败，请检查统一日志后重试";
  }
}

async function submit() {
  if (step.value === "url") {
    await inspectRepository();
  } else if (step.value === "preview") {
    await installPlugin();
  }
}

watch(selectedPluginId, resetPluginReview);
watch(
  () => props.open,
  (open) => {
    if (!open) {
      resetDialog();
    }
  },
);
onBeforeUnmount(clearDeveloperToken);
</script>

<template>
  <ResponsiveDialog
    :open="open"
    title="添加插件仓库"
    description="检查公开仓库并选择要安装的插件。"
    @update:open="handleOpen"
  >
    <form
      :id="formId"
      class="grid gap-4"
      autocomplete="off"
      @submit.prevent="submit"
    >
      <Field v-if="step === 'url'">
        <FieldLabel for="repository-url">GitHub 公共仓库地址</FieldLabel>
        <Input
          id="repository-url"
          v-model="repositoryUrl"
          name="repository-url"
          type="url"
          maxlength="512"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          placeholder="粘贴公开仓库地址"
        />
        <FieldDescription>
          Core 会读取默认分支的仓库索引并锁定 Commit SHA。
        </FieldDescription>
      </Field>

      <RepositoryPreview
        v-else-if="preview"
        v-model:selected-plugin-id="selectedPluginId"
        v-model:scope-grant-decisions="scopeGrantDecisions"
        :preview="preview"
      />

      <section
        v-if="step !== 'url' && requiresRiskGrant && selectedPlugin"
        class="grid gap-3 border-t border-border pt-4"
        aria-labelledby="lifecycle-risk-title"
      >
        <Alert>
          <TriangleAlert class="text-status-warning" aria-hidden="true" />
          <AlertTitle id="lifecycle-risk-title">安装脚本风险授权</AlertTitle>
          <AlertDescription>
            {{ selectedPlugin.lifecycleScriptReason }}
          </AlertDescription>
        </Alert>

        <Field orientation="horizontal">
          <Checkbox
            id="allow-lifecycle-scripts"
            v-model="riskApproved"
            :disabled="!developmentMode || step === 'installing'"
          />
          <FieldLabel class="font-normal" for="allow-lifecycle-scripts">
            我明确允许本次 Commit 执行 npm 安装脚本
          </FieldLabel>
        </Field>

        <Field v-if="developmentMode">
          <FieldLabel for="audiodown-developer-token">
            开发者令牌
          </FieldLabel>
          <Input
            id="audiodown-developer-token"
            v-model="developerToken"
            type="password"
            name="audiodown-developer-token"
            autocomplete="off"
            autocapitalize="none"
            spellcheck="false"
            :disabled="step === 'installing'"
          />
        </Field>
        <Alert v-else variant="destructive">
          <AlertTitle>开发者模式未启用</AlertTitle>
          <AlertDescription>
            当前只能检查仓库，不能授权执行安装脚本。
          </AlertDescription>
        </Alert>
      </section>

      <div
        v-if="step === 'installing'"
        class="grid gap-2"
        role="status"
        aria-label="正在安装插件"
      >
        <Skeleton class="h-3 w-40" />
        <Skeleton class="h-8 w-full" />
      </div>

      <Alert v-if="error" variant="destructive">
        <TriangleAlert aria-hidden="true" />
        <AlertTitle>操作失败</AlertTitle>
        <AlertDescription>{{ error }}</AlertDescription>
      </Alert>
    </form>

    <template #footer>
      <Button
        type="button"
        variant="outline"
        :disabled="step === 'installing'"
        @click="step === 'url' ? closeDialog() : goBack()"
      >
        {{ step === "url" ? "取消" : "返回" }}
      </Button>
      <Button
        type="submit"
        :form="formId"
        :disabled="
          inspecting ||
          step === 'installing' ||
          (step === 'preview' && installDisabled)
        "
        @click.prevent="submit"
      >
        {{
          inspecting
            ? "检查中"
            : step === "url"
              ? "检查仓库"
              : step === "installing"
                ? "安装中"
                : "安装插件"
        }}
      </Button>
    </template>
  </ResponsiveDialog>
</template>
