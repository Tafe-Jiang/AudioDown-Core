<script setup lang="ts">
import { computed } from "vue";
import { CheckCircle2, LockKeyhole, TriangleAlert } from "@lucide/vue";

import type {
  RepositoryPluginPreview,
  RepositoryPreview,
} from "@/api/client";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";

const props = defineProps<{
  preview: RepositoryPreview;
  selectedPluginId: string;
  scopeGrantDecisions: Record<string, boolean>;
}>();

const emit = defineEmits<{
  "update:selectedPluginId": [pluginId: string];
  "update:scopeGrantDecisions": [decisions: Record<string, boolean>];
}>();

function selected(plugin: RepositoryPluginPreview) {
  return plugin.pluginId === props.selectedPluginId;
}

const selectedPlugin = computed(
  () =>
    props.preview.plugins.find(
      (plugin) => plugin.pluginId === props.selectedPluginId,
    ) ?? null,
);
const hasCredentialDeclarations = computed(() => {
  const credentials = selectedPlugin.value?.credentials;
  return (
    credentials !== undefined &&
    (credentials.providedScopes.length > 0 ||
      credentials.requiredScopes.length > 0 ||
      credentials.optionalScopes.length > 0)
  );
});

function decisionKey(access: "required" | "optional", scope: string) {
  return `${access}:${scope}`;
}

function updateDecision(
  access: "required" | "optional",
  scope: string,
  value: boolean | "indeterminate",
) {
  emit("update:scopeGrantDecisions", {
    ...props.scopeGrantDecisions,
    [decisionKey(access, scope)]: value === true,
  });
}
</script>

<template>
  <div class="grid gap-4">
    <div class="grid gap-1 border-b border-border pb-3">
      <div class="flex flex-wrap items-center gap-2">
        <h3 class="text-sm font-semibold">{{ preview.repository.name }}</h3>
        <Badge variant="outline">
          <LockKeyhole aria-hidden="true" />
          {{ preview.repository.commitSha.slice(0, 7) }}
        </Badge>
      </div>
      <p class="break-all text-xs text-muted-foreground">
        {{ preview.repository.sourceUrl }}
      </p>
    </div>

    <div class="grid gap-2" aria-label="仓库插件">
      <Button
        v-for="plugin in preview.plugins"
        :key="plugin.pluginId"
        type="button"
        variant="outline"
        class="h-auto w-full justify-start px-3 py-3 text-left"
        :aria-pressed="selected(plugin)"
        :data-selected="selected(plugin)"
        @click="$emit('update:selectedPluginId', plugin.pluginId)"
      >
        <CheckCircle2
          v-if="selected(plugin)"
          class="text-primary"
          aria-hidden="true"
        />
        <TriangleAlert
          v-else-if="plugin.requiresLifecycleScriptGrant"
          class="text-status-warning"
          aria-hidden="true"
        />
        <span v-else class="size-4" aria-hidden="true" />
        <span class="grid min-w-0 flex-1 gap-1">
          <span class="truncate font-medium">{{ plugin.name }}</span>
          <span class="flex flex-wrap items-center gap-2 text-xs">
            <Badge variant="secondary">{{ plugin.pluginType }}</Badge>
            <span class="text-muted-foreground">{{ plugin.version }}</span>
            <span v-if="plugin.alreadyInstalled" class="text-muted-foreground">
              已安装
            </span>
          </span>
        </span>
      </Button>
    </div>

    <section
      v-if="selectedPlugin && hasCredentialDeclarations"
      class="grid gap-3 border-t border-border pt-4"
      aria-labelledby="credential-declarations-title"
    >
      <div class="flex items-center gap-2">
        <LockKeyhole class="size-4 text-muted-foreground" aria-hidden="true" />
        <h4 id="credential-declarations-title" class="text-sm font-semibold">
          凭据作用域
        </h4>
      </div>
      <p class="text-xs leading-5 text-muted-foreground">
        凭据声明不等于授权。安装后仍需绑定具体凭据和精确 Origin，插件才能使用账号能力。
      </p>

      <div
        v-if="selectedPlugin.credentials.providedScopes.length > 0"
        class="grid gap-2"
      >
        <p class="text-xs font-medium">提供的作用域</p>
        <div
          v-for="declaration in selectedPlugin.credentials.providedScopes"
          :key="`provided:${declaration.scope}`"
          class="grid gap-1 border-l-2 border-border pl-3"
        >
          <span class="text-sm font-medium">{{ declaration.scope }}</span>
          <span
            v-for="origin in declaration.targetOrigins"
            :key="origin"
            class="break-all font-mono text-xs text-muted-foreground"
          >
            {{ origin }}
          </span>
        </div>
      </div>

      <div
        v-if="
          selectedPlugin.credentials.requiredScopes.length > 0 ||
          selectedPlugin.credentials.optionalScopes.length > 0
        "
        class="grid gap-2"
      >
        <p class="text-xs font-medium">请求的作用域</p>
        <label
          v-for="declaration in selectedPlugin.credentials.requiredScopes"
          :key="`required:${declaration.scope}`"
          class="flex items-start gap-3 border-b border-border py-2"
        >
          <Checkbox
            :id="`scope-required-${declaration.scope}`"
            :data-scope-decision="decisionKey('required', declaration.scope)"
            :model-value="
              scopeGrantDecisions[
                decisionKey('required', declaration.scope)
              ] === true
            "
            @update:model-value="
              updateDecision('required', declaration.scope, $event)
            "
          />
          <span class="grid min-w-0 flex-1 gap-1">
            <span class="flex flex-wrap items-center gap-2">
              <span class="text-sm font-medium">{{ declaration.scope }}</span>
              <Badge variant="secondary">必需</Badge>
            </span>
            <span
              v-for="origin in declaration.targetOrigins"
              :key="origin"
              class="break-all font-mono text-xs text-muted-foreground"
            >
              {{ origin }}
            </span>
          </span>
        </label>
        <label
          v-for="declaration in selectedPlugin.credentials.optionalScopes"
          :key="`optional:${declaration.scope}`"
          class="flex items-start gap-3 border-b border-border py-2"
        >
          <Checkbox
            :id="`scope-optional-${declaration.scope}`"
            :data-scope-decision="decisionKey('optional', declaration.scope)"
            :model-value="
              scopeGrantDecisions[
                decisionKey('optional', declaration.scope)
              ] === true
            "
            @update:model-value="
              updateDecision('optional', declaration.scope, $event)
            "
          />
          <span class="grid min-w-0 flex-1 gap-1">
            <span class="flex flex-wrap items-center gap-2">
              <span class="text-sm font-medium">{{ declaration.scope }}</span>
              <Badge variant="outline">可选</Badge>
            </span>
            <span
              v-for="origin in declaration.targetOrigins"
              :key="origin"
              class="break-all font-mono text-xs text-muted-foreground"
            >
              {{ origin }}
            </span>
          </span>
        </label>
      </div>
    </section>
  </div>
</template>
