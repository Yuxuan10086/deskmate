<template>
  <aside class="chat-panel" :class="{ collapsed }">
    <!-- 折叠态：细栏 + 展开按钮 -->
    <button v-if="collapsed" class="expand-tab" title="展开同桌小轩" @click="toggleCollapse">
      💬
    </button>
    <template v-else>
      <div class="cp-head">
        <span class="cp-avatar">轩</span>
        <div class="cp-title-wrap">
          <div class="cp-title">同桌小轩</div>
          <div class="cp-scope">{{ scopeHint }}</div>
        </div>
        <button class="cp-collapse" title="折叠" @click="toggleCollapse">»</button>
      </div>

      <div ref="msgsBody" class="cp-msgs" @scroll="onScroll">
        <div v-if="currentThread.length === 0" class="cp-empty">{{ emptyHint }}</div>
        <div
          v-for="(m, i) in currentThread"
          :key="i"
          class="cp-msg"
          :class="m.role === 'user' ? 'me' : 'ai'"
        >
          <template v-if="m.role === 'user'">
            <div class="cp-bubble">
              <img v-if="m.image" :src="m.image" class="cp-img" />
              <div v-if="m.content">{{ m.content }}</div>
            </div>
          </template>
          <template v-else>
            <div class="cp-bubble">
              <!-- AI 回复渲染 markdown，避免用户看到一堆 # * - 语法符号 -->
              <div v-if="m.content" class="cp-text md" v-html="renderMd(m.content)"></div>
              <div v-else class="cp-typing">{{ busy ? "思考中…" : "" }}</div>
              <em v-if="m.saved" class="cp-saved">✓ 已记入课堂笔记</em>
            </div>
          </template>
        </div>
      </div>

      <div v-if="stickTop === false" class="cp-jump" @click="scrollBottom(true)">↓ 最新</div>

      <!-- S12：小轩请求记入笔记的确认卡 -->
      <div v-if="noteReq" class="cp-notecard">
        <div class="cp-nc-head">
          <span>📌 小轩想把这条内容记入课堂笔记</span>
        </div>
        <div class="cp-nc-title">{{ noteReq.title }}</div>
        <pre class="cp-nc-body">{{ noteReq.content }}</pre>
        <div class="cp-nc-actions">
          <button class="cp-nc-yes" :disabled="noteReqBusy" @click="answerNote(true)">
            记入笔记
          </button>
          <button class="cp-nc-no" :disabled="noteReqBusy" @click="answerNote(false)">
            不用
          </button>
        </div>
      </div>

      <div class="cp-input">
        <img
          v-if="pendingImage"
          :src="pendingImage"
          class="cp-thumb"
          title="点击移除图片"
          @click="pendingImage = null"
        />
        <button class="cp-plus" title="上传图片或文件" @click="pickFile">＋</button>
        <input
          ref="fileInput"
          type="file"
          class="cp-file"
          @change="onFilePicked"
        />
        <textarea
          v-model="draft"
          rows="1"
          :placeholder="inputHint"
          @paste="onPaste"
          @keydown.enter.exact.prevent="send"
        ></textarea>
        <button class="cp-send" :disabled="busy" @click="send">{{ busy ? "…" : "发送" }}</button>
      </div>
      <div class="cp-foot">Enter 发送 · 点 ＋ 或 Ctrl+V 上传图片/文件</div>
    </template>
  </aside>
</template>

<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { api, deskmateChatStream, errMsg } from "../api/ipc";
import { useDataStore } from "../stores/data";
import type { ChatScope, NoteSaveRequestEvent } from "../types";

marked.setOptions({ gfm: true, breaks: true });

/** 把 AI 回复的 markdown 渲染成 HTML（经 DOMPurify 消毒后交给 v-html）。 */
function renderMd(src: string): string {
  try {
    const html = marked.parse(src, { async: false }) as string;
    return DOMPurify.sanitize(html);
  } catch {
    return src;
  }
}

interface PanelMsg {
  role: "user" | "assistant";
  content: string;
  image?: string;
  saved?: boolean;
}

const route = useRoute();
const store = useDataStore();

/* -------------------------- 按路由感知聊天范围 -------------------------- */
const scopeInfo = computed<{ scope: ChatScope; courseId: number | null; sessionId: number | null; key: string }>(() => {
  if (route.name === "session-detail") {
    const sid = Number(route.params.id);
    return { scope: "session", courseId: null, sessionId: sid, key: `session:${sid}` };
  }
  if (route.name === "course-detail") {
    const cid = Number(route.params.id);
    return { scope: "course", courseId: cid, sessionId: null, key: `course:${cid}` };
  }
  return { scope: "dashboard", courseId: null, sessionId: null, key: "dashboard" };
});

const sessionOfScope = computed(() =>
  scopeInfo.value.sessionId != null
    ? store.sessions.find((s) => s.id === scopeInfo.value.sessionId)
    : undefined
);
const courseOfScope = computed(() =>
  scopeInfo.value.courseId != null
    ? store.courses.find((c) => c.id === scopeInfo.value.courseId)
    : sessionOfScope.value
      ? store.courseById(sessionOfScope.value.courseId)
      : undefined
);

const scopeHint = computed(() => {
  switch (scopeInfo.value.scope) {
    case "session":
      return sessionOfScope.value?.status === "recording"
        ? "上课中 · 可实时提问"
        : `本节课${courseOfScope.value ? ` · ${courseOfScope.value.name}` : ""}`;
    case "course":
      return courseOfScope.value ? `本门课 · ${courseOfScope.value.name}` : "本门课程";
    default:
      return "所有课程 · 按需查询";
  }
});
const emptyHint = computed(() => {
  switch (scopeInfo.value.scope) {
    case "session":
      return sessionOfScope.value?.status === "recording"
        ? "上课时有没听懂的地方直接问我，我会结合当前字幕回答；板书可以拍照、截图或点＋上传。"
        : "关于这节课的内容（字幕、笔记、行动项）都可以问我。";
    case "course":
      return "关于这门课的课堂记录、笔记与花费，直接问我就好。";
    default:
      return "问我任何课程相关的问题：课程安排、课堂内容、笔记、花费……我会自己去查数据再回答。";
  }
});
const inputHint = computed(() => {
  switch (scopeInfo.value.scope) {
    case "session":
      return "针对本节课提问…";
    case "course":
      return "针对本门课提问…";
    default:
      return "问我任何课程相关的问题…";
  }
});

/* -------------------------- 每个范围独立消息线程 -------------------------- */
const threads = ref<Record<string, PanelMsg[]>>({});
const currentThread = computed(() => threads.value[scopeInfo.value.key] ?? []);

function ensureThread(key: string): PanelMsg[] {
  if (!threads.value[key]) threads.value[key] = [];
  return threads.value[key];
}

/* ------------------------------ 折叠/滚动 ------------------------------ */
const COLLAPSE_KEY = "chatpanel-collapsed";
const collapsed = ref(localStorage.getItem(COLLAPSE_KEY) === "1");
function toggleCollapse() {
  collapsed.value = !collapsed.value;
  localStorage.setItem(COLLAPSE_KEY, collapsed.value ? "1" : "0");
}

const msgsBody = ref<HTMLElement | null>(null);
const stickTop = ref(true);
/** 必须先于下方 watch 声明：watch 源 getter 立即执行，引用后文 const 会触发 TDZ */
const busy = ref(false);

function onScroll() {
  const el = msgsBody.value;
  if (!el) return;
  stickTop.value = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
}
function scrollBottom(force = false) {
  const el = msgsBody.value;
  if (el && (stickTop.value || force)) el.scrollTop = el.scrollHeight;
}
watch(
  () => [scopeInfo.value.key, currentThread.value.length, busy.value],
  async () => {
    await nextTick();
    scrollBottom();
  }
);

/* ------------------------------ 发送与粘贴 ------------------------------ */
const draft = ref("");
const pendingImage = ref<string | null>(null);
const fileInput = ref<HTMLInputElement | null>(null);
/** 待用户确认的「记入笔记」请求（后端在等待答复，一次最多一个） */
const noteReq = ref<NoteSaveRequestEvent | null>(null);
const noteReqBusy = ref(false);

function fileToDataUrl(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}

/** ＋ 按钮：打开文件选择器 */
function pickFile() {
  fileInput.value?.click();
}

/** 选中文件：图片挂为待发图片；其他文本类文件读入输入框 */
async function onFilePicked(e: Event) {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  input.value = "";
  if (!f) return;
  await ingestFile(f);
}

async function ingestFile(f: File) {
  if (f.type.startsWith("image/") || /\.(png|jpe?g|gif|webp|bmp)$/i.test(f.name)) {
    if (f.size > 8 * 1024 * 1024) {
      alert("图片请控制在 8MB 以内");
      return;
    }
    pendingImage.value = await fileToDataUrl(f);
    return;
  }
  if (f.size > 512 * 1024) {
    alert("文本文件请控制在 512KB 以内（图片不受此限）");
    return;
  }
  try {
    const t = await f.text();
    draft.value += (draft.value ? "\n" : "") + `【文件：${f.name}】\n${t}`;
  } catch {
    alert("该文件不是可读取的文本文件；图片可直接上传，其他类型请到课堂页「附件」上传。");
  }
}

/** Ctrl+V：图片直接挂为附件；文件（非图片）读文本拼入草稿；纯文本走默认粘贴。 */
function onPaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  if (!items) return;
  for (const item of items) {
    if (item.type.startsWith("image/")) {
      const f = item.getAsFile();
      if (f) {
        e.preventDefault();
        void ingestFile(f);
      }
      return;
    }
  }
  for (const item of items) {
    if (item.kind === "file") {
      const f = item.getAsFile();
      if (f && !f.type.startsWith("image/")) {
        e.preventDefault();
        void ingestFile(f);
      }
      return;
    }
  }
}

/** 答复「记入笔记」确认卡 */
async function answerNote(approved: boolean) {
  const req = noteReq.value;
  if (!req || noteReqBusy.value) return;
  noteReqBusy.value = true;
  try {
    await api.respondNoteSave(req.requestId, approved);
    noteReq.value = null;
  } catch (e) {
    alert(`操作失败：${errMsg(e)}`);
  } finally {
    noteReqBusy.value = false;
  }
}

async function send() {
  if (busy.value) return;
  const text = draft.value.trim();
  const image = pendingImage.value;
  if (!text && !image) return;
  const { scope, courseId, sessionId, key } = scopeInfo.value;
  const list = ensureThread(key);
  list.push({ role: "user", content: text, image: image ?? undefined });
  draft.value = "";
  pendingImage.value = null;
  // 占位的 AI 气泡：流式 delta 直接写入它。
  // 必须取「响应式代理」而不是 push 进去的原始对象——直接改原始对象不会触发视图更新，
  // 表现为流式输出完全不生效、直到结束时 busy 翻转才一次性显示全文。
  list.push({ role: "assistant", content: "" });
  const aiMsg = list[list.length - 1];
  busy.value = true;
  await nextTick();
  scrollBottom(true);
  try {
    // 后端约定：history 不含当前这轮（用户消息 + 占位回复），最多带 24 条
    const history = list
      .slice(0, -2)
      .slice(-24)
      .filter((m) => m.content.trim())
      .map((m) => ({ role: m.role, content: m.content }));
    const res = await deskmateChatStream(
      { scope, courseId, sessionId, history, text, imageDataUrl: image },
      (ev) => {
        if (ev.type === "delta") {
          aiMsg.content += ev.text;
          void nextTick().then(() => scrollBottom());
        } else if (ev.type === "noteRequest") {
          noteReq.value = {
            requestId: ev.requestId,
            sessionId: ev.sessionId,
            title: ev.title,
            content: ev.content,
          };
        }
      }
    );
    aiMsg.content = res.reply || aiMsg.content;
    aiMsg.saved = res.savedToNote;
    await store.reloadSessions();
    await store.reloadUsage();
  } catch (e) {
    // 后端错误以字符串形式抛出，必须用 errMsg 取值，否则只会显示 undefined
    aiMsg.content = `调用失败：${errMsg(e)}`;
  } finally {
    busy.value = false;
    await nextTick();
    scrollBottom(true);
  }
}
</script>

<style scoped>
.chat-panel {
  position: relative;
  width: 320px;
  flex-shrink: 0;
  background: #fff;
  border-left: 1px solid var(--line);
  display: flex;
  flex-direction: column;
  height: 100%;
}
.chat-panel.collapsed {
  width: 44px;
  align-items: center;
}
.expand-tab {
  width: 100%;
  height: 44px;
  border: none;
  background: transparent;
  font-size: 18px;
  cursor: pointer;
  border-radius: 0;
}
.expand-tab:hover {
  background: var(--brand-soft);
}

.cp-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 14px 14px 10px;
  border-bottom: 1px solid var(--line);
}
.cp-avatar {
  width: 34px;
  height: 34px;
  border-radius: 10px;
  background: linear-gradient(135deg, var(--brand), #7c5cff);
  color: #fff;
  font-weight: 700;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 15px;
  flex-shrink: 0;
}
.cp-title-wrap {
  flex: 1;
  min-width: 0;
}
.cp-title {
  font-size: 14px;
  font-weight: 700;
}
.cp-scope {
  font-size: 11px;
  color: var(--ink-mute);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cp-collapse {
  border: none;
  background: transparent;
  font-size: 16px;
  color: var(--ink-mute);
  cursor: pointer;
  padding: 4px 6px;
  border-radius: 6px;
}
.cp-collapse:hover {
  background: #eef1f7;
  color: var(--ink);
}

.cp-msgs {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 12px 14px;
}
.cp-empty {
  font-size: 12px;
  color: var(--ink-mute);
  line-height: 1.8;
  padding: 18px 4px;
}
.cp-msg {
  display: flex;
}
.cp-msg.me {
  justify-content: flex-end;
}
.cp-bubble {
  max-width: 85%;
  padding: 8px 12px;
  border-radius: 12px;
  font-size: 13px;
  line-height: 1.7;
  background: #f1f4fa;
  color: var(--ink);
  white-space: pre-wrap;
  word-break: break-word;
}
.cp-msg.me .cp-bubble {
  background: var(--brand);
  color: #fff;
}
.cp-text {
  white-space: pre-wrap;
}
/* markdown 渲染：块级元素间不要出现 pre-wrap 产生的空行 */
.cp-text.md {
  white-space: normal;
}
.cp-text.md :deep(p) {
  margin: 0 0 6px;
}
.cp-text.md :deep(p:last-child) {
  margin-bottom: 0;
}
.cp-text.md :deep(h1),
.cp-text.md :deep(h2),
.cp-text.md :deep(h3),
.cp-text.md :deep(h4) {
  margin: 8px 0 4px;
  font-size: 13px;
  font-weight: 700;
  line-height: 1.5;
}
.cp-text.md :deep(h1:first-child),
.cp-text.md :deep(h2:first-child),
.cp-text.md :deep(h3:first-child) {
  margin-top: 0;
}
.cp-text.md :deep(ul),
.cp-text.md :deep(ol) {
  margin: 4px 0 6px;
  padding-left: 18px;
}
.cp-text.md :deep(li) {
  margin: 2px 0;
}
.cp-text.md :deep(li > p) {
  margin: 0;
}
.cp-text.md :deep(code) {
  background: rgba(15, 23, 42, 0.07);
  padding: 1px 4px;
  border-radius: 4px;
  font-size: 12px;
  font-family: ui-monospace, Consolas, "Courier New", monospace;
}
.cp-text.md :deep(pre) {
  background: rgba(15, 23, 42, 0.05);
  padding: 8px 10px;
  border-radius: 8px;
  overflow-x: auto;
  margin: 6px 0;
}
.cp-text.md :deep(pre code) {
  background: transparent;
  padding: 0;
}
.cp-text.md :deep(blockquote) {
  margin: 6px 0;
  padding: 2px 10px;
  border-left: 3px solid #cbd5e1;
  color: var(--ink-mute);
}
.cp-text.md :deep(a) {
  color: var(--brand);
  word-break: break-all;
}
.cp-text.md :deep(table) {
  border-collapse: collapse;
  margin: 6px 0;
  font-size: 12px;
  display: block;
  max-width: 100%;
  overflow-x: auto;
}
.cp-text.md :deep(th),
.cp-text.md :deep(td) {
  border: 1px solid #e2e8f0;
  padding: 3px 8px;
  text-align: left;
}
.cp-text.md :deep(hr) {
  border: none;
  border-top: 1px solid #e2e8f0;
  margin: 8px 0;
}
.cp-img {
  max-width: 200px;
  border-radius: 8px;
  display: block;
  margin-bottom: 6px;
}
.cp-saved {
  display: inline-block;
  margin-top: 6px;
  font-size: 11px;
  color: #15803d;
  font-style: normal;
}
.cp-typing {
  color: var(--ink-mute);
}

.cp-jump {
  position: absolute;
  right: 18px;
  bottom: 96px;
  font-size: 11px;
  padding: 4px 10px;
  border-radius: 999px;
  background: var(--brand);
  color: #fff;
  cursor: pointer;
  box-shadow: 0 4px 12px rgba(37, 99, 235, 0.35);
}

/* ------------------------------ 笔记确认卡 ------------------------------ */
.cp-notecard {
  margin: 0 12px 6px;
  padding: 10px 12px;
  border: 1px solid #f59e0b;
  background: #fffbeb;
  border-radius: 10px;
}
.cp-nc-head {
  font-size: 12px;
  font-weight: 700;
  color: #92400e;
}
.cp-nc-title {
  margin-top: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--ink);
}
.cp-nc-body {
  margin: 6px 0 0;
  max-height: 130px;
  overflow-y: auto;
  font-family: inherit;
  font-size: 12px;
  line-height: 1.6;
  color: var(--ink-mute);
  white-space: pre-wrap;
  word-break: break-word;
}
.cp-nc-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
  justify-content: flex-end;
}
.cp-nc-yes,
.cp-nc-no {
  border: none;
  border-radius: 8px;
  padding: 5px 12px;
  font-size: 12px;
  cursor: pointer;
}
.cp-nc-yes {
  background: var(--brand);
  color: #fff;
}
.cp-nc-no {
  background: #e5e7eb;
  color: var(--ink);
}
.cp-nc-yes:disabled,
.cp-nc-no:disabled {
  opacity: 0.5;
}

.cp-input {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  padding: 10px 12px 4px;
}
.cp-thumb {
  width: 40px;
  height: 40px;
  object-fit: cover;
  border-radius: 8px;
  cursor: pointer;
  border: 1px solid var(--line);
}
.cp-plus {
  width: 32px;
  height: 32px;
  flex-shrink: 0;
  border: 1px solid var(--line);
  background: #f6f8fc;
  color: var(--ink-mute);
  border-radius: 8px;
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
}
.cp-plus:hover {
  border-color: var(--brand);
  color: var(--brand);
}
.cp-file {
  display: none;
}
.cp-input textarea {
  flex: 1;
  resize: none;
  border: 1px solid var(--line);
  border-radius: 10px;
  padding: 8px 11px;
  font: inherit;
  font-size: 13px;
  max-height: 110px;
  outline: none;
}
.cp-input textarea:focus {
  border-color: var(--brand);
}
.cp-send {
  border: none;
  background: var(--brand);
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  padding: 8px 12px;
  border-radius: 10px;
  cursor: pointer;
  flex-shrink: 0;
}
.cp-send:disabled {
  opacity: 0.5;
  cursor: default;
}
.cp-foot {
  padding: 4px 14px 10px;
  font-size: 10px;
  color: var(--ink-mute);
  text-align: center;
}
</style>
