<script setup lang="ts">
import { onMounted, ref } from "vue";
import { Compass } from "@lucide/vue";
import { useRouter } from "vue-router";

import { api, type EmptyStateResponse } from "../api/client";
import AsyncState from "../components/common/AsyncState.vue";
import EmptyState from "../components/common/EmptyState.vue";
import PageHeader from "../components/common/PageHeader.vue";

const router = useRouter();
const state = ref<EmptyStateResponse | null>(null);
const error = ref("");
const loading = ref(false);

async function load() {
  loading.value = true;
  error.value = "";
  try {
    state.value = await api.discover();
  } catch {
    error.value = "无法读取发现状态";
  } finally {
    loading.value = false;
  }
}

onMounted(load);
</script>

<template>
  <section class="grid gap-5">
    <PageHeader
      title="发现"
      description="内容插件提供的发现频道会显示在这里。"
    />

    <AsyncState
      :loading="loading"
      :error="error"
      :empty="Boolean(state)"
      @retry="load"
    >
      <template #empty>
        <EmptyState
          v-if="state"
          class="min-h-[420px] border-0 bg-transparent"
          :title="state.title"
          description="安装内容插件后，发现频道会显示在这里。"
          :primary-label="state.actionLabel"
          @primary="router.push('/plugins')"
        >
          <template #icon>
            <Compass aria-hidden="true" />
          </template>
        </EmptyState>
      </template>
    </AsyncState>
  </section>
</template>
