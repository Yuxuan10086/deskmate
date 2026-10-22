/** 桌面通知：Tauri 环境走 notification 插件，浏览器 mock 走 window.Notification（若可用）。 */

import { isTauri } from "../api/ipc";

let permissionChecked = false;

async function ensurePermission(): Promise<boolean> {
  if (permissionChecked) return true;
  permissionChecked = true;
  if (isTauri()) {
    try {
      const notif = await import("@tauri-apps/plugin-notification");
      const perm = await notif.isPermissionGranted();
      if (!perm) {
        const r = await notif.requestPermission();
        return r === "granted";
      }
      return true;
    } catch {
      return false;
    }
  }
  // 浏览器环境（mock）
  if (typeof window !== "undefined" && "Notification" in window) {
    if (Notification.permission === "default") {
      await Notification.requestPermission();
    }
    return Notification.permission === "granted";
  }
  return false;
}

export async function notify(title: string, body: string): Promise<void> {
  const ok = await ensurePermission();
  if (!ok) return;
  if (isTauri()) {
    try {
      const notif = await import("@tauri-apps/plugin-notification");
      await notif.sendNotification({ title, body });
    } catch (err) {
      console.warn("Tauri 通知发送失败", err);
    }
  } else if (typeof window !== "undefined" && "Notification" in window) {
    try {
      new Notification(title, { body });
    } catch {
      /* noop */
    }
  }
}

/** 预算通知节流器：同一 session 内同阈值只发一次 */
export const budgetNotifier = {
  fired80: false,
  fired100: false,
  reset() {
    this.fired80 = false;
    this.fired100 = false;
  },
  async check(pct: number) {
    if (pct >= 100 && !this.fired100) {
      this.fired100 = true;
      await notify("预算已超支", `本月已用 ${pct}%，请关注花费`);
    } else if (pct >= 80 && !this.fired80) {
      this.fired80 = true;
      await notify("预算提醒", `本月已用 ${pct}%，请留意模型档位`);
    }
  },
};
