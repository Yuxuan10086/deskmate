import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { router } from "./router";
import { useAppStore } from "./stores/app";
import { useDataStore } from "./stores/data";
import "./style.css";

async function bootstrap() {
  const app = createApp(App);
  const pinia = createPinia();
  app.use(pinia);

  // 首屏前加载设置与全部数据（mock 模式会在首次访问时生成种子数据）
  const appStore = useAppStore(pinia);
  const dataStore = useDataStore(pinia);
  const [settingsErr, dataErr] = await Promise.allSettled([
    appStore.load(),
    dataStore.loadAll(),
  ]);
  if (settingsErr.status === "rejected" || dataErr.status === "rejected") {
    console.error("启动数据加载失败", { settingsErr, dataErr });
  }

  app.use(router);
  app.mount("#app");
}

void bootstrap();
