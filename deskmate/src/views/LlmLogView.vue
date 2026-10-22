<template>
  <div class="page log-page">
    <div class="page-head">
      <button class="btn ghost sm" @click="router.back()">← 返回</button>
      <h1>大模型调用日志</h1>
      <span class="sub">记录每次调用的输入与输出，最多保留最近 1000 条</span>
      <div class="spacer"></div>
      <select v-model="stageFilter" class="select max-160">
        <option value="">全部环节</option>
        <option v-for="s in stageOptions" :key="s" :value="s">{{ stageName(s) }}</option>
      </select>
      <select v-model="okFilter" class="select max-120">
        <option value="">全部状态</option>
        <option value="ok">成功</option>
        <option value="fail">失败</option>
      </select>
      <button class="btn ghost sm" :disabled="busy" @click="load">刷新</button>
      <button class="btn ghost sm danger" :disabled="busy || logs.length === 0" @click="clearAll">
        清空日志
      </button>
    </div>

    <div v-if="loading" class="card empty-card">加载中…</div>
    <div v-else-if="filtered.length === 0" class="card empty-card">
      {{ logs.length === 0 ? "暂无调用日志。发起一次模型调用后会记录在这里。" : "当前筛选条件下无日志" }}
    </div>

    <div v-else class="log-list">
      <div v-for="log in filtered" :key="log.id" class="card log-item" :class="{ fail: !log.ok }">
        <div class="log-head" @click="toggle(log.id)">
          <span class="state-dot" :class="log.ok ? 'ok' : 'no'"></span>
          <span class="log-ts">{{ fmtTime(log.ts) }}</span>
          <span class="tag stage">{{ stageName(log.stage) }}</span>
          <span class="log-model">{{ log.model }}</span>
          <span v-if="log.sessionId != null" class="muted">课堂 #{{ log.sessionId }}</span>
          <span class="muted tokens">
            入 {{ log.inputTokens }}<template v-if="log.cachedTokens > 0">（缓存 {{ log.cachedTokens }}）</template>
            · 出 {{ log.outputTokens }}
          </span>
          <span v-if="!log.ok" class="err-text">失败</span>
          <div class="spacer"></div>
          <span class="chev" :class="{ open: expanded.has(log.id) }">▾</span>
        </div>
        <div v-if="expanded.has(log.id)" class="log-body">
          <div v-if="log.error" class="io-block">
            <div class="io-title err-text">错误</div>
            <pre>{{ log.error }}</pre>
          </div>
          <div v-if="log.systemPrompt" class="io-block">
            <div class="io-title">系统提示词</div>
            <pre>{{ log.systemPrompt }}</pre>
          </div>
          <div v-if="log.userPrompt" class="io-block">
            <div class="io-title">输入</div>
            <pre>{{ log.userPrompt }}</pre>
          </div>
          <div v-if="log.response" class="io-block">
            <div class="io-title">输出</div>
            <pre>{{ log.response }}</pre>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { api } from "../api/ipc";
import type { LlmLog } from "../types";

const router = useRouter();

const logs = ref<LlmLog[]>([]);
const loading = ref(true);
const busy = ref(false);
const expanded = ref<Set<number>>(new Set());
const stageFilter = ref("");
const okFilter = ref("");

const stageOptions = computed(() => [...new Set(logs.value.map((l) => l.stage))]);

const filtered = computed(() =>
  logs.value.filter((l) => {
    if (stageFilter.value && l.stage !== stageFilter.value) return false;
    if (okFilter.value === "ok" && !l.ok) return false;
    if (okFilter.value === "fail" && l.ok) return false;
    return true;
  })
);

const STAGE_NAMES: Record<string, string> = {
  asr: "语音转写",
  ast: "同声传译",
  translate: "实时翻译",
  translate_batch: "归档翻译",
  tidy: "文稿整理",
  detect: "实时行动项",
  detect_batch: "归档行动项",
  speaker: "说话人标注",
  archive: "归档（笔记+行动项）",
  note: "笔记生成",
  chat: "课堂对话",
};

function stageName(stage: string): string {
  return STAGE_NAMES[stage] ?? stage;
}

function fmtTime(ts: string): string {
  const d = new Date(ts);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getMonth() + 1}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

function toggle(id: number) {
  if (expanded.value.has(id)) {
    expanded.value.delete(id);
  } else {
    expanded.value.add(id);
  }
  // 触发响应式
  expanded.value = new Set(expanded.value);
}

async function load() {
  busy.value = true;
  try {
    logs.value = await api.listLlmLogs(1000);
  } catch (e) {
    alert(`读取日志失败：${e}`);
  } finally {
    busy.value = false;
    loading.value = false;
  }
}

async function clearAll() {
  if (!confirm("确定清空全部调用日志？此操作不可恢复。")) return;
  busy.value = true;
  try {
    await api.clearLlmLogs();
    logs.value = [];
    expanded.value = new Set();
  } catch (e) {
    alert(`清空失败：${e}`);
  } finally {
    busy.value = false;
  }
}

onMounted(load);
</script>

<style scoped>
.log-page {
  /* 最小宽度：日志列表过窄时改由外层出横向滚动条 */
  min-width: 820px;
}
.page-head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.page-head h1 {
  margin: 0;
}
.sub {
  font-size: 12px;
  color: var(--ink-mute);
}
.spacer {
  flex: 1;
}
.max-160 {
  max-width: 160px;
}
.max-120 {
  max-width: 120px;
}
.empty-card {
  padding: 40px;
  text-align: center;
  color: var(--ink-mute);
  font-size: 13px;
}
.log-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.log-item {
  padding: 0;
  overflow: hidden;
}
.log-item.fail {
  border-left: 3px solid #c2410c;
}
.log-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px 16px;
  cursor: pointer;
  user-select: none;
  font-size: 13px;
}
.log-head:hover {
  background: color-mix(in srgb, var(--ink) 3%, transparent);
}
.state-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.state-dot.ok {
  background: #0f9d58;
}
.state-dot.no {
  background: #c2410c;
}
.log-ts {
  color: var(--ink-soft);
  font-variant-numeric: tabular-nums;
}
.tag.stage {
  font-size: 11px;
  font-weight: 600;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  padding: 2px 8px;
  border-radius: 999px;
}
.log-model {
  font-weight: 600;
}
.muted {
  color: var(--ink-mute);
  font-size: 12px;
}
.tokens {
  font-variant-numeric: tabular-nums;
}
.err-text {
  color: #c2410c;
  font-weight: 600;
  font-size: 12px;
}
.chev {
  color: var(--ink-mute);
  transition: transform 0.15s;
}
.chev.open {
  transform: rotate(180deg);
}
.log-body {
  border-top: 1px solid var(--line);
  padding: 12px 16px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.io-block .io-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--ink-soft);
  margin-bottom: 4px;
}
.io-block pre {
  margin: 0;
  padding: 10px 12px;
  background: color-mix(in srgb, var(--ink) 4%, transparent);
  border-radius: 8px;
  font-size: 12px;
  line-height: 1.6;
  white-space: pre-wrap;
  word-break: break-word;
  max-height: 280px;
  overflow: auto;
  font-family: inherit;
}
.danger {
  color: #c2410c;
}
</style>
