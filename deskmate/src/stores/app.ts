import { defineStore } from "pinia";
import { api } from "../api/ipc";
import type { AppSettings } from "../types";

export const useAppStore = defineStore("app", {
  state: () =>
    ({
      settings: null as AppSettings | null,
      loaded: false,
    }),
  actions: {
    async load() {
      this.settings = await api.getSettings();
      this.loaded = true;
    },
    async save(patch: Partial<AppSettings>) {
      this.settings = await api.saveSettings(patch);
    },
  },
});
