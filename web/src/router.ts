import { createRouter, createWebHistory } from "vue-router";

import DiscoverView from "./views/DiscoverView.vue";
import LogsView from "./views/LogsView.vue";
import PluginsView from "./views/PluginsView.vue";
import SearchView from "./views/SearchView.vue";
import SystemView from "./views/SystemView.vue";

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", redirect: "/discover" },
    { path: "/discover", component: DiscoverView },
    { path: "/search", component: SearchView },
    { path: "/plugins", component: PluginsView },
    { path: "/logs", component: LogsView },
    { path: "/system", component: SystemView },
  ],
});

export default router;
