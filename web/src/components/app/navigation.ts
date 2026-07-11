import {
  Compass,
  Plug,
  ScrollText,
  Search,
  Settings2,
} from "@lucide/vue";

export const navigation = [
  { to: "/discover", label: "发现", icon: Compass },
  { to: "/search", label: "搜索", icon: Search },
  { to: "/plugins", label: "插件", icon: Plug },
  { to: "/logs", label: "日志", icon: ScrollText },
  { to: "/system", label: "系统", icon: Settings2 },
] as const;
