<script setup lang="ts">
import { onMounted, ref } from "vue";
import { LoaderCircle, Search } from "@lucide/vue";
import { useRouter } from "vue-router";

import { api, type EmptyStateResponse } from "../api/client";
import AsyncState from "../components/common/AsyncState.vue";
import EmptyState from "../components/common/EmptyState.vue";
import PageHeader from "../components/common/PageHeader.vue";
import { Button } from "../components/ui/button";
import { Field, FieldLabel } from "../components/ui/field";
import { Input } from "../components/ui/input";

const router = useRouter();
const query = ref("");
const state = ref<EmptyStateResponse | null>(null);
const error = ref("");
const loading = ref(false);

async function search() {
  loading.value = true;
  error.value = "";
  try {
    state.value = await api.search(query.value);
  } catch {
    error.value = "无法读取搜索状态";
  } finally {
    loading.value = false;
  }
}

onMounted(search);
</script>

<template>
  <section class="grid gap-5">
    <PageHeader
      title="搜索"
      description="搜索已安装内容插件提供的内容。"
    />

    <form class="w-full max-w-2xl" @submit.prevent="search">
      <Field>
        <FieldLabel class="sr-only" for="search-query">
          搜索内容
        </FieldLabel>
        <div class="flex w-full items-stretch gap-2">
          <Input
            id="search-query"
            v-model="query"
            class="h-9"
            type="search"
            placeholder="输入关键词"
          />
          <Button class="h-9 shrink-0" type="submit" :disabled="loading">
            <LoaderCircle
              v-if="loading"
              class="animate-spin"
              aria-hidden="true"
            />
            <Search v-else aria-hidden="true" />
            {{ loading ? "查询中" : "搜索" }}
          </Button>
        </div>
      </Field>
    </form>

    <AsyncState
      :loading="loading && !state && !error"
      :error="error"
      :empty="Boolean(state)"
      @retry="search"
    >
      <template #empty>
        <EmptyState
          v-if="state"
          class="min-h-[360px] border-0 bg-transparent"
          :title="state.title"
          description="安装内容插件后即可执行聚合搜索。"
          :primary-label="state.actionLabel"
          @primary="router.push('/plugins')"
        >
          <template #icon>
            <Search aria-hidden="true" />
          </template>
        </EmptyState>
      </template>
    </AsyncState>
  </section>
</template>
