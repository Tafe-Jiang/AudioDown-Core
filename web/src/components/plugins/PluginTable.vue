<script setup lang="ts">
import { computed, nextTick, reactive, ref } from "vue";
import { TriangleAlert } from "@lucide/vue";
import { toast } from "vue-sonner";

import PluginActionsMenu from "./PluginActionsMenu.vue";
import PluginSettingsSheet from "./PluginSettingsSheet.vue";
import {
  api,
  type PluginItem,
  type PluginSettings,
} from "@/api/client";
import StatusBadge, {
  type StatusTone,
} from "@/components/common/StatusBadge.vue";
import {
  Alert,
  AlertDescription,
  AlertTitle,
} from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Button } from "@/components/ui/button";

const props = defineProps<{
  items: PluginItem[];
  supervisorAvailable: boolean;
}>();

const emit = defineEmits<{
  "items-refreshed": [items: PluginItem[]];
}>();

const busy = reactive<Record<string, boolean>>({});
const errors = reactive<Record<string, string>>({});
const enabledOverrides = reactive<Record<string, boolean | undefined>>({});
const settingsPlugin = ref<PluginItem | null>(null);
const settingsReturnFocusId = ref("");
const uninstallPlugin = ref<PluginItem | null>(null);
const settingsOpen = computed(() => settingsPlugin.value !== null);

function pluginBusy(pluginId: string) {
  return busy[pluginId] === true;
}

function pluginEnabled(plugin: PluginItem) {
  return enabledOverrides[plugin.pluginId] ?? plugin.enabled;
}

function displayPlugin(plugin: PluginItem): PluginItem {
  return { ...plugin, enabled: pluginEnabled(plugin) };
}

function statusPresentation(status: string): {
  label: string;
  tone: StatusTone;
} {
  switch (status) {
    case "running":
    case "healthy":
      return { label: "运行中", tone: "success" };
    case "starting":
      return { label: "启动中", tone: "warning" };
    case "stopping":
      return { label: "停止中", tone: "warning" };
    case "disabled":
      return { label: "已禁用", tone: "warning" };
    case "failed":
    case "unhealthy":
      return { label: "异常", tone: "danger" };
    case "stopped":
      return { label: "已停止", tone: "neutral" };
    case "installed":
      return { label: "已安装", tone: "neutral" };
    default:
      return { label: status, tone: "neutral" };
  }
}

function modeLabel(mode: PluginItem["runMode"]) {
  return mode === "always" ? "持续运行" : "按需运行";
}

function setError(pluginId: string, message = "") {
  errors[pluginId] = message;
}

async function refreshItems() {
  const result = await api.plugins();
  emit("items-refreshed", result.items);
}

async function toggleEnabled(plugin: PluginItem, enabled: boolean) {
  if (pluginBusy(plugin.pluginId) || !props.supervisorAvailable) {
    return;
  }
  const previous = pluginEnabled(plugin);
  enabledOverrides[plugin.pluginId] = enabled;
  busy[plugin.pluginId] = true;
  setError(plugin.pluginId);
  try {
    const updated = await api.updatePlugin(plugin.pluginId, {
      enabled,
      runMode: plugin.runMode,
      priority: plugin.priority,
    });
    enabledOverrides[plugin.pluginId] = updated.enabled;
    await refreshItems();
    toast.success(enabled ? "插件已启用" : "插件已禁用");
  } catch {
    enabledOverrides[plugin.pluginId] = previous;
    setError(plugin.pluginId, "更新插件失败，请检查统一日志后重试");
  } finally {
    busy[plugin.pluginId] = false;
  }
}

async function runCommand(plugin: PluginItem, command: "start" | "stop") {
  if (pluginBusy(plugin.pluginId) || !props.supervisorAvailable) {
    return;
  }
  busy[plugin.pluginId] = true;
  setError(plugin.pluginId);
  try {
    if (command === "start") {
      await api.startPlugin(plugin.pluginId);
    } else {
      await api.stopPlugin(plugin.pluginId);
    }
    await refreshItems();
    toast.success(command === "start" ? "插件已启动" : "插件已停止");
  } catch {
    setError(
      plugin.pluginId,
      command === "start"
        ? "启动插件失败，请检查统一日志后重试"
        : "停止插件失败，请检查统一日志后重试",
    );
  } finally {
    busy[plugin.pluginId] = false;
  }
}

function openSettings(plugin: PluginItem, returnFocusId: string) {
  settingsPlugin.value = displayPlugin(plugin);
  settingsReturnFocusId.value = returnFocusId;
  setError(plugin.pluginId);
}

function closeSettings() {
  const returnFocusId = settingsReturnFocusId.value;
  settingsPlugin.value = null;
  settingsReturnFocusId.value = "";
  void nextTick(() => {
    document.getElementById(returnFocusId)?.focus();
  });
}

async function saveSettings(settings: PluginSettings) {
  const plugin = settingsPlugin.value;
  if (!plugin || pluginBusy(plugin.pluginId) || !props.supervisorAvailable) {
    return;
  }
  busy[plugin.pluginId] = true;
  setError(plugin.pluginId);
  try {
    await api.updatePlugin(plugin.pluginId, settings);
    await refreshItems();
    closeSettings();
    toast.success("插件设置已保存");
  } catch {
    setError(plugin.pluginId, "保存插件设置失败，请检查统一日志后重试");
  } finally {
    busy[plugin.pluginId] = false;
  }
}

async function confirmUninstall() {
  const plugin = uninstallPlugin.value;
  if (!plugin || pluginBusy(plugin.pluginId) || !props.supervisorAvailable) {
    return;
  }
  busy[plugin.pluginId] = true;
  setError(plugin.pluginId);
  try {
    await api.uninstallPlugin(plugin.pluginId);
    await refreshItems();
    uninstallPlugin.value = null;
    toast.success("插件已卸载");
  } catch {
    setError(plugin.pluginId, "卸载插件失败，请检查统一日志后重试");
  } finally {
    busy[plugin.pluginId] = false;
  }
}
</script>

<template>
  <div class="grid min-w-0 gap-3">
    <div
      data-desktop-plugin-table
      class="hidden min-w-0 overflow-hidden rounded-md border border-border md:block"
    >
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>名称</TableHead>
            <TableHead>类型</TableHead>
            <TableHead>版本</TableHead>
            <TableHead>状态</TableHead>
            <TableHead>启用</TableHead>
            <TableHead>运行模式</TableHead>
            <TableHead>优先级</TableHead>
            <TableHead class="text-right">操作</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <template v-for="plugin in items" :key="plugin.pluginId">
            <TableRow>
              <TableCell>
                <div class="min-w-0">
                  <strong class="block truncate text-sm">
                    {{ plugin.name }}
                  </strong>
                  <span class="block truncate text-xs text-muted-foreground">
                    {{ plugin.pluginId }}
                  </span>
                </div>
              </TableCell>
              <TableCell>{{ plugin.pluginType }}</TableCell>
              <TableCell>{{ plugin.version }}</TableCell>
              <TableCell>
                <StatusBadge
                  :tone="statusPresentation(plugin.status).tone"
                  :label="statusPresentation(plugin.status).label"
                />
              </TableCell>
              <TableCell>
                <div class="flex items-center gap-2">
                  <Switch
                    :id="`enabled-${plugin.pluginId}`"
                    :model-value="pluginEnabled(plugin)"
                    :disabled="
                      pluginBusy(plugin.pluginId) || !supervisorAvailable
                    "
                    :data-plugin-enable="plugin.pluginId"
                    :aria-label="`${plugin.name} 启用状态`"
                    @update:model-value="toggleEnabled(plugin, $event)"
                  />
                  <Label
                    :for="`enabled-${plugin.pluginId}`"
                    class="sr-only"
                  >
                    {{ plugin.name }} 启用状态
                  </Label>
                </div>
              </TableCell>
              <TableCell>{{ modeLabel(plugin.runMode) }}</TableCell>
              <TableCell>{{ plugin.priority }}</TableCell>
              <TableCell>
                <PluginActionsMenu
                  :plugin="displayPlugin(plugin)"
                  :busy="pluginBusy(plugin.pluginId)"
                  :supervisor-available="supervisorAvailable"
                  context="desktop"
                  @start="runCommand(plugin, 'start')"
                  @stop="runCommand(plugin, 'stop')"
                  @settings="openSettings(plugin, $event)"
                  @uninstall="uninstallPlugin = plugin"
                />
              </TableCell>
            </TableRow>
            <TableRow v-if="errors[plugin.pluginId]">
              <TableCell :colspan="8">
                <Alert
                  variant="destructive"
                  :data-plugin-error="plugin.pluginId"
                >
                  <TriangleAlert aria-hidden="true" />
                  <AlertTitle>操作失败</AlertTitle>
                  <AlertDescription>
                    {{ errors[plugin.pluginId] }}
                  </AlertDescription>
                </Alert>
              </TableCell>
            </TableRow>
          </template>
        </TableBody>
      </Table>
    </div>

    <article
      v-for="plugin in items"
      :key="`mobile-${plugin.pluginId}`"
      :data-mobile-plugin-item="plugin.pluginId"
      class="grid min-w-0 gap-3 border-b border-border py-3 md:hidden"
    >
      <header class="flex min-w-0 items-start justify-between gap-3">
        <div class="min-w-0">
          <strong class="block truncate text-sm">{{ plugin.name }}</strong>
          <span class="block truncate text-xs text-muted-foreground">
            {{ plugin.pluginType }} · {{ plugin.version }}
          </span>
          <span class="block break-all text-xs text-muted-foreground">
            {{ plugin.pluginId }}
          </span>
        </div>
        <StatusBadge
          :tone="statusPresentation(plugin.status).tone"
          :label="statusPresentation(plugin.status).label"
        />
      </header>
      <dl class="grid grid-cols-2 gap-3 text-sm">
        <div>
          <dt class="text-xs text-muted-foreground">运行模式</dt>
          <dd>{{ modeLabel(plugin.runMode) }}</dd>
        </div>
        <div>
          <dt class="text-xs text-muted-foreground">优先级</dt>
          <dd>{{ plugin.priority }}</dd>
        </div>
      </dl>
      <div class="flex items-center justify-between gap-3">
        <div class="flex items-center gap-2">
          <Switch
            :model-value="pluginEnabled(plugin)"
            :disabled="pluginBusy(plugin.pluginId) || !supervisorAvailable"
            :data-mobile-plugin-enable="plugin.pluginId"
            :aria-label="`${plugin.name} 启用状态`"
            @update:model-value="toggleEnabled(plugin, $event)"
          />
          <span class="text-sm">
            {{ pluginEnabled(plugin) ? "已启用" : "已禁用" }}
          </span>
        </div>
        <PluginActionsMenu
          :plugin="displayPlugin(plugin)"
          :busy="pluginBusy(plugin.pluginId)"
          :supervisor-available="supervisorAvailable"
          context="mobile"
          @start="runCommand(plugin, 'start')"
          @stop="runCommand(plugin, 'stop')"
          @settings="openSettings(plugin, $event)"
          @uninstall="uninstallPlugin = plugin"
        />
      </div>
      <Alert
        v-if="errors[plugin.pluginId]"
        variant="destructive"
        :data-mobile-plugin-error="plugin.pluginId"
      >
        <TriangleAlert aria-hidden="true" />
        <AlertTitle>操作失败</AlertTitle>
        <AlertDescription>{{ errors[plugin.pluginId] }}</AlertDescription>
      </Alert>
    </article>

    <PluginSettingsSheet
      :open="settingsOpen"
      :plugin="settingsPlugin"
      :busy="
        settingsPlugin ? pluginBusy(settingsPlugin.pluginId) : false
      "
      :error="settingsPlugin ? errors[settingsPlugin.pluginId] ?? '' : ''"
      @update:open="!$event && closeSettings()"
      @save="saveSettings"
    />

    <AlertDialog
      :open="uninstallPlugin !== null"
      @update:open="!$event && (uninstallPlugin = null)"
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            卸载 {{ uninstallPlugin?.name }}？
          </AlertDialogTitle>
          <AlertDialogDescription>
            Supervisor 会先停止并移除插件运行资源，再删除安装记录。
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>取消</AlertDialogCancel>
          <Button
            type="button"
            variant="destructive"
            :disabled="
              uninstallPlugin
                ? pluginBusy(uninstallPlugin.pluginId)
                : true
            "
            @click="confirmUninstall"
          >
            确认卸载
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>
</template>
