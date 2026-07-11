import { readonly, ref } from "vue";

import { api, type SystemResponse } from "@/api/client";

const system = ref<SystemResponse | null>(null);
const loading = ref(false);
const error = ref<string | null>(null);
let started = false;
let pending: Promise<void> | null = null;

async function loadSystemStatus(): Promise<void> {
  if (pending) {
    return pending;
  }

  loading.value = true;
  error.value = null;
  pending = api
    .system()
    .then((response) => {
      system.value = response;
    })
    .catch(() => {
      error.value = "无法读取系统状态";
    })
    .finally(() => {
      loading.value = false;
      pending = null;
    });

  return pending;
}

async function refresh(): Promise<void> {
  return loadSystemStatus();
}

export function useSystemStatus() {
  if (!started) {
    started = true;
    void loadSystemStatus();
  }

  return {
    system: readonly(system),
    loading: readonly(loading),
    error: readonly(error),
    refresh,
  };
}
