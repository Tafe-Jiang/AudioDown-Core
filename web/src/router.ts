import { createRouter, createWebHistory } from "vue-router";

import AlbumView from "./views/AlbumView.vue";
import DiscoverView from "./views/DiscoverView.vue";
import LogsView from "./views/LogsView.vue";
import PluginsView from "./views/PluginsView.vue";
import SearchView from "./views/SearchView.vue";
import SystemView from "./views/SystemView.vue";

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: "/", redirect: "/discover" },
    {
      path: "/discover",
      component: DiscoverView,
      meta: { title: "发现" },
    },
    {
      path: "/albums/detail",
      name: "album",
      component: AlbumView,
      meta: { title: "专辑详情" },
    },
    { path: "/search", component: SearchView, meta: { title: "搜索" } },
    { path: "/plugins", component: PluginsView, meta: { title: "插件" } },
    { path: "/logs", component: LogsView, meta: { title: "日志" } },
    { path: "/system", component: SystemView, meta: { title: "系统" } },
  ],
});

export default router;
