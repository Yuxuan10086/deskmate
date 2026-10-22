<template>
  <div class="page tasks-page">
    <header class="page-head">
      <div>
        <h1>待办事项</h1>
        <p class="page-sub">
          课堂里老师布置的任务点「加入」会自动出现在这里，也可以手动添加。按 DDL
          排序，没有截止时间的排在最前面。
        </p>
      </div>
    </header>

    <!-- 手动添加 -->
    <section class="add-card">
      <input
        v-model="newTitle"
        class="add-title"
        placeholder="添加一个待办，例如：交实验报告"
        @keydown.enter="addTodo"
      />
      <input v-model="newDdl" type="date" class="add-ddl" title="截止日期（可留空）" />
      <button class="btn btn-primary" :disabled="!newTitle.trim()" @click="addTodo">添加</button>
    </section>

    <!-- 未完成 -->
    <section class="todo-section">
      <h2 class="sec-title">进行中 · {{ openTodos.length }}</h2>
      <div v-if="openTodos.length === 0" class="empty-line">暂无待办，上完课记得回来看看有没有新任务。</div>
      <div v-for="t in openTodos" :key="t.id" class="todo-item">
        <input
          type="checkbox"
          class="todo-check"
          :checked="false"
          title="标记完成"
          @change="toggleDone(t, true)"
        />
        <div class="todo-main">
          <div class="todo-title-line">
            <span class="todo-title">{{ t.title }}</span>
            <span
              v-if="t.sessionId != null"
              class="todo-session"
              title="跳转到来源课堂"
              @click="openSession(t.sessionId)"
            >
              {{ sessionName(t.sessionId) }}
            </span>
          </div>
          <div v-if="t.detail" class="todo-detail">{{ t.detail }}</div>
          <div class="todo-meta">
            <span v-if="t.source === 'ai'" class="tag tag-ai">课堂识别</span>
            <span v-else class="tag tag-manual">手动添加</span>
            <label class="ddl-edit" :class="ddlState(t.ddl)">
              <span class="ddl-label">{{ ddlText(t.ddl) }}</span>
              <input
                type="date"
                class="ddl-input"
                :value="ddlInputValue(t.ddl)"
                @change="changeDdl(t, ($event.target as HTMLInputElement).value)"
              />
            </label>
          </div>
        </div>
        <button class="todo-del" title="删除" @click="remove(t)">删除</button>
      </div>
    </section>

    <!-- 已完成 -->
    <section v-if="doneTodos.length > 0" class="todo-section">
      <h2 class="sec-title sec-title-done" @click="showDone = !showDone">
        已完成 · {{ doneTodos.length }} <span class="fold">{{ showDone ? "▾" : "▸" }}</span>
      </h2>
      <template v-if="showDone">
        <div v-for="t in doneTodos" :key="t.id" class="todo-item todo-done">
          <input
            type="checkbox"
            class="todo-check"
            checked
            title="恢复为未完成"
            @change="toggleDone(t, false)"
          />
          <div class="todo-main">
            <div class="todo-title-line">
              <span class="todo-title">{{ t.title }}</span>
              <span
                v-if="t.sessionId != null"
                class="todo-session"
                @click="openSession(t.sessionId)"
              >
                {{ sessionName(t.sessionId) }}
              </span>
            </div>
            <div class="todo-meta">
              <span v-if="t.completedAt" class="done-at">完成于 {{ fmtDateTime(t.completedAt) }}</span>
            </div>
          </div>
          <button class="todo-del" title="删除" @click="remove(t)">删除</button>
        </div>
      </template>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { api, errMsg } from "../api/ipc";
import { useDataStore } from "../stores/data";
import type { Todo } from "../types";

const router = useRouter();
const store = useDataStore();

const newTitle = ref("");
const newDdl = ref("");
const showDone = ref(true);

/** 后端已按「未完成→DDL 未知→DDL 升序」排序，这里分组即可 */
const openTodos = computed(() => store.todos.filter((t) => !t.done));
const doneTodos = computed(() =>
  store.todos.filter((t) => t.done).slice(0, 100)
);

async function addTodo() {
  const title = newTitle.value.trim();
  if (!title) return;
  try {
    await api.createTodo({ title, ddl: newDdl.value || null });
    await store.reloadTodos();
    newTitle.value = "";
    newDdl.value = "";
  } catch (e) {
    alert(`添加失败：${errMsg(e)}`);
  }
}

async function toggleDone(t: Todo, done: boolean) {
  try {
    await api.setTodoDone(t.id, done);
    await store.reloadTodos();
  } catch (e) {
    alert(`操作失败：${errMsg(e)}`);
  }
}

async function remove(t: Todo) {
  if (!confirm(`删除待办「${t.title}」？`)) return;
  try {
    await api.deleteTodo(t.id);
    await store.reloadTodos();
  } catch (e) {
    alert(`删除失败：${errMsg(e)}`);
  }
}

async function changeDdl(t: Todo, ymd: string) {
  try {
    await api.updateTodo({ id: t.id, ddl: ymd || null });
    await store.reloadTodos();
  } catch (e) {
    alert(`修改 DDL 失败：${errMsg(e)}`);
  }
}

function sessionName(sid: number): string {
  return store.sessions.find((s) => s.id === sid)?.title ?? `课堂 #${sid}`;
}
function openSession(sid: number | null) {
  if (sid == null) return;
  void router.push({ name: "session-detail", params: { id: sid } });
}

/** 每次进入待办页都重新拉取，避免其他页面改动后列表过期。 */
onMounted(() => {
  void store.reloadTodos();
});

/* ------------------------------ DDL 展示 ------------------------------ */

function parseDate(v: string): Date | null {
  const d = new Date(v);
  return Number.isNaN(d.getTime()) ? null : d;
}

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** date input 需要 YYYY-MM-DD；RFC3339 转本地日期，模型给的自然语言无法回填则空 */
function ddlInputValue(v: string | null): string {
  if (!v) return "";
  const d = parseDate(v);
  return d ? `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}` : "";
}

function ddlText(v: string | null): string {
  if (!v) return "📅 DDL 未知 · 点击设置";
  const d = parseDate(v);
  if (!d) return `📅 ${v}（点击改为日期）`;
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const day = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const diffDays = Math.round((day.getTime() - today.getTime()) / 86400000);
  const base = `${d.getMonth() + 1}月${d.getDate()}日`;
  if (diffDays === 0) return `📅 今天截止`;
  if (diffDays === 1) return `📅 明天截止`;
  if (diffDays === -1) return `📅 昨天（已过期）`;
  if (diffDays > 1 && diffDays <= 7) return `📅 ${base} · ${diffDays} 天后`;
  return `📅 ${base}${diffDays < 0 ? " · 已过期" : ""}`;
}

function ddlState(v: string | null): string {
  if (!v) return "ddl-unknown";
  const d = parseDate(v);
  if (!d) return "ddl-unknown";
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const day = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const diffDays = Math.round((day.getTime() - today.getTime()) / 86400000);
  if (diffDays < 0) return "ddl-overdue";
  if (diffDays <= 3) return "ddl-soon";
  return "ddl-ok";
}

function fmtDateTime(v: string): string {
  const d = parseDate(v);
  if (!d) return v;
  return `${d.getMonth() + 1}-${pad2(d.getDate())} ${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}
</script>

<style scoped>
.tasks-page {
  /* 不限最大宽度：随外层窗口拉伸；仅保底最小宽度，过窄时由外层出横向滚动条 */
  min-width: 620px;
}
.page-head {
  margin-bottom: 18px;
}
.page-sub {
  margin-top: 6px;
  color: var(--ink-mute);
  font-size: 13px;
  line-height: 1.7;
}

.add-card {
  display: flex;
  gap: 10px;
  align-items: center;
  background: #fff;
  border: 1px solid var(--line);
  border-radius: 12px;
  padding: 12px 14px;
  margin-bottom: 22px;
}
.add-title {
  flex: 1;
  border: none;
  outline: none;
  font: inherit;
  font-size: 14px;
}
.add-ddl {
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 6px 8px;
  font: inherit;
  font-size: 13px;
}

.todo-section {
  margin-bottom: 24px;
}
.sec-title {
  font-size: 14px;
  font-weight: 700;
  margin: 0 0 10px;
  color: var(--ink);
}
.sec-title-done {
  color: var(--ink-mute);
  cursor: pointer;
  user-select: none;
}
.fold {
  font-size: 12px;
  margin-left: 4px;
}
.empty-line {
  font-size: 13px;
  color: var(--ink-mute);
  padding: 8px 2px;
}

.todo-item {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  background: #fff;
  border: 1px solid var(--line);
  border-radius: 12px;
  padding: 12px 14px;
  margin-bottom: 8px;
}
.todo-check {
  margin-top: 3px;
  width: 16px;
  height: 16px;
  cursor: pointer;
  flex-shrink: 0;
}
.todo-main {
  flex: 1;
  min-width: 0;
}
.todo-title-line {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.todo-title {
  font-size: 14px;
  font-weight: 600;
  line-height: 1.5;
}
.todo-session {
  font-size: 11px;
  color: var(--brand);
  background: var(--brand-soft);
  border-radius: 6px;
  padding: 1px 8px;
  cursor: pointer;
}
.todo-session:hover {
  text-decoration: underline;
}
.todo-detail {
  margin-top: 4px;
  font-size: 12px;
  color: var(--ink-mute);
  line-height: 1.6;
  white-space: pre-wrap;
}
.todo-meta {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
}
.tag {
  font-size: 10px;
  border-radius: 6px;
  padding: 1px 7px;
}
.tag-ai {
  background: #ede9fe;
  color: #6d28d9;
}
.tag-manual {
  background: #eef2f7;
  color: var(--ink-mute);
}
.ddl-edit {
  position: relative;
  font-size: 12px;
  border-radius: 6px;
  padding: 1px 8px;
  cursor: pointer;
}
.ddl-label {
  position: relative;
  z-index: 1;
}
.ddl-input {
  position: absolute;
  inset: 0;
  width: 100%;
  opacity: 0;
  cursor: pointer;
}
.ddl-unknown {
  background: #f1f4f9;
  color: var(--ink-mute);
}
.ddl-ok {
  background: #ecfdf5;
  color: #047857;
}
.ddl-soon {
  background: #fff7ed;
  color: #c2410c;
}
.ddl-overdue {
  background: #fef2f2;
  color: #dc2626;
  font-weight: 600;
}
.todo-del {
  border: none;
  background: transparent;
  color: var(--ink-mute);
  cursor: pointer;
  font-size: 12px;
  padding: 4px;
}
.todo-del:hover {
  color: var(--danger);
}
.todo-done .todo-title {
  text-decoration: line-through;
  color: var(--ink-mute);
  font-weight: 500;
}
.done-at {
  font-size: 11px;
  color: var(--ink-mute);
}
</style>
