<template>
  <div class="shell">
    <aside class="sidebar">
      <div class="brand">
        <span class="brand-mark">轩</span>
        <span class="brand-name">同桌小轩</span>
      </div>

      <!-- 开始上课：按系统时间自动匹配课程并预填 -->
      <div class="sidebar-actions">
        <button class="btn start-btn" @click="goStart">＋ 开始上课</button>
        <button v-if="recordingSession" class="btn back-class-btn" @click="goRecording">
          ● 回到课堂
        </button>
      </div>

      <nav class="nav">
        <RouterLink v-for="item in navItems" :key="item.to" :to="item.to" class="nav-item">
          <span class="nav-icon">{{ item.icon }}</span>
          <span class="nav-label">{{ item.label }}</span>
          <span v-if="item.to === '/tasks' && openTodoCount > 0" class="nav-badge">
            {{ openTodoCount }}
          </span>
        </RouterLink>
      </nav>
      <div class="sidebar-foot">
        <div class="dot" :class="{ on: !isTauriEnv }"></div>
        {{ isTauriEnv ? "桌面模式" : "浏览器预览（本地示例数据）" }}
      </div>
    </aside>
    <main class="content">
      <slot />
    </main>
    <!-- 右侧同桌小轩聊天板块：按路由自动切换上下文范围 -->
    <ChatPanel />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted } from "vue";
import { RouterLink, useRouter } from "vue-router";
import { api, isTauri } from "../api/ipc";
import { useDataStore } from "../stores/data";
import ChatPanel from "../components/ChatPanel.vue";

const router = useRouter();
const store = useDataStore();

const navItems = [
  { to: "/courses", label: "课程", icon: "📚" },
  { to: "/schedule", label: "课程表", icon: "🗓️" },
  { to: "/tasks", label: "待办", icon: "✅" },
  { to: "/cost", label: "花费", icon: "💰" },
  { to: "/settings", label: "设置", icon: "⚙️" },
];

const isTauriEnv = isTauri();

/** 未完成待办数（侧栏角标）。 */
const openTodoCount = computed(() => store.todos.filter((t) => !t.done).length);

/** 当前正在录制的课堂（用于侧栏「回到课堂」一键返回）。 */
const recordingSession = computed(() =>
  store.sessions.find((s) => s.status === "recording")
);

function goStart() {
  void router.push({ name: "new-session" });
}
function goRecording() {
  if (recordingSession.value) {
    void router.push({ name: "session-detail", params: { id: recordingSession.value.id } });
  }
}

onMounted(async () => {
  try {
    await store.loadAll();
  } catch (e) {
    console.warn("[layout] 数据加载失败", e);
  }
  // 桌面端：以后端采集状态为准校正「回到课堂」按钮
  if (isTauriEnv) {
    try {
      const st = await api.captureStatus();
      if (st.running && st.sessionId != null && !recordingSession.value) {
        await store.reloadSessions();
      }
    } catch {
      /* 忽略 */
    }
  }
});
</script>

<style scoped>
.shell {
  display: flex;
  height: 100%;
}
.sidebar {
  width: var(--sidebar-w);
  background: #fff;
  border-right: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
}
.brand {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 22px 20px 14px;
}
.brand-mark {
  width: 32px;
  height: 32px;
  border-radius: 9px;
  background: linear-gradient(135deg, var(--brand), #7c5cff);
  color: #fff;
  font-weight: 700;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 16px;
}
.brand-name {
  font-size: 18px;
  font-weight: 700;
  letter-spacing: 1px;
}
.sidebar-actions {
  padding: 4px 12px 12px;
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.start-btn {
  width: 100%;
  font-weight: 600;
}
.back-class-btn {
  width: 100%;
  color: var(--danger);
  border-color: rgba(239, 68, 68, 0.35);
  background: rgba(239, 68, 68, 0.06);
  font-weight: 600;
}
.back-class-btn:hover {
  background: rgba(239, 68, 68, 0.12);
}
.nav {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px 10px;
}
.nav-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 9px 12px;
  border-radius: 8px;
  color: var(--ink-soft);
  font-size: 14px;
  font-weight: 500;
  transition: all 0.15s;
}
.nav-item:hover {
  background: var(--brand-soft);
  color: var(--brand);
}
.nav-item.router-link-active {
  background: var(--brand-soft);
  color: var(--brand);
  font-weight: 600;
}
/* 固定图标盒尺寸 + 统一基线，消除 emoji 大小/对齐不齐 */
.nav-icon {
  width: 22px;
  height: 22px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 17px;
  line-height: 1;
  flex-shrink: 0;
}
.nav-label {
  line-height: 22px;
}
.nav-badge {
  margin-left: auto;
  min-width: 20px;
  height: 18px;
  padding: 0 6px;
  border-radius: 999px;
  background: var(--danger);
  color: #fff;
  font-size: 11px;
  font-weight: 600;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.sidebar-foot {
  margin-top: auto;
  padding: 14px 18px;
  font-size: 11px;
  color: var(--ink-mute);
  display: flex;
  align-items: center;
  gap: 6px;
}
.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent);
}
.dot.on {
  background: var(--warn);
}
/* 唯一滚动容器：纵向常态滚动，横向仅在页面内容超出最小宽度时出现滚动条 */
.content {
  flex: 1;
  min-width: 0;
  overflow: auto;
}
</style>
