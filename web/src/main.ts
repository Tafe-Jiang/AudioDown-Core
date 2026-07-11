import { createApp, defineComponent, Fragment, h } from "vue";

import App from "./App.vue";
import { Toaster } from "./components/ui/sonner";
import router from "./router";
import "./styles.css";

const RootApp = defineComponent({
  name: "RootApp",
  setup() {
    return () => h(Fragment, [h(App), h(Toaster)]);
  },
});

createApp(RootApp).use(router).mount("#app");
