<template>
  <div class="page cost-page">
    <div class="page-head">
      <h1>花费管理</h1>
      <span class="sub">数据来自每次模型调用的用量明细，估算金额以硅基流动账单为准</span>
      <div class="spacer"></div>
      <div class="range-switch">
        <button
          v-for="r in ranges"
          :key="r.id"
          class="rs-btn"
          :class="{ active: range === r.id }"
          @click="range = r.id"
        >
          {{ r.label }}
        </button>
      </div>
    </div>

    <div class="kpis">
      <div class="kpi card">
        <div class="kpi-label">{{ range === "month" ? "本月花费" : "区间花费" }}</div>
        <div class="kpi-val">¥{{ totalCost.toFixed(2) }}</div>
        <div v-if="range === 'month'" class="kpi-sub">
          预算 ¥{{ budget }} · 已用 {{ budgetPct }}%
        </div>
        <div class="budget" v-if="range === 'month'">
          <i :style="{ width: budgetPct + '%' }" :class="{ over: budgetPct >= 100 }"></i>
        </div>
      </div>
      <div class="kpi card">
        <div class="kpi-label">涉及课堂</div>
        <div class="kpi-val">{{ sessionCount }} <small>节</small></div>
        <div class="kpi-sub">上课总时长 {{ hours.toFixed(1) }} 小时</div>
      </div>
      <div class="kpi card">
        <div class="kpi-label">模型调用</div>
        <div class="kpi-val">{{ records.length.toLocaleString() }} <small>次</small></div>
        <div class="kpi-sub">免费调用 {{ freeCount }} 次（{{ freePct }}%）</div>
      </div>
      <div class="kpi card">
        <div class="kpi-label">Token 用量</div>
        <div class="kpi-val">{{ tokenTextVal }}</div>
        <div class="kpi-sub">缓存命中 {{ cachedText }} · ASR {{ asrHours.toFixed(1) }}h</div>
      </div>
    </div>

    <div class="panels">
      <div class="card panel">
        <div class="p-title">按功能环节</div>
        <div v-for="row in byStage" :key="row.key" class="bar-row">
          <span>{{ row.label }}</span>
          <div class="bar"><i :style="{ width: row.pct + '%' }"></i></div>
          <b>¥{{ row.amount.toFixed(2) }}</b>
        </div>
      </div>
      <div class="card panel">
        <div class="p-title">按模型系列</div>
        <div v-for="row in byTier" :key="row.key" class="bar-row">
          <span>{{ row.emoji }} {{ row.label }}</span>
          <div class="bar" :class="row.key">
            <i :style="{ width: row.pct + '%' }"></i>
          </div>
          <b>¥{{ row.amount.toFixed(2) }}</b>
        </div>
        <div class="p-note">缓存命中为检测环节节省了约 ¥{{ savedByCache.toFixed(2) }}</div>
      </div>
    </div>

    <div class="panels">
      <div class="card panel">
        <div class="p-title">按课程</div>
        <table class="mini-tbl">
          <thead>
            <tr><th>课程</th><th>课堂数</th><th>时长(h)</th><th>花费</th><th>均价/节</th></tr>
          </thead>
          <tbody>
            <tr v-for="row in byCourse" :key="row.courseId ?? 'none'">
              <td>{{ row.name }}</td>
              <td>{{ row.count }}</td>
              <td>{{ row.hours.toFixed(1) }}</td>
              <td>¥{{ row.amount.toFixed(2) }}</td>
              <td>¥{{ (row.amount / row.count).toFixed(2) }}</td>
            </tr>
            <tr v-if="byCourse.length === 0">
              <td colspan="5" class="mini-empty">当前区间暂无数据</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div class="card panel">
        <div class="p-title">近 7 日趋势</div>
        <div class="trend">
          <div v-for="(d, i) in trend7" :key="i" class="t-col" :class="{ today: d.isToday }">
            <b>¥{{ d.amount.toFixed(1) }}</b>
            <i :style="{ height: Math.max(2, (d.amount / trendMax) * 100) + '%' }"></i>
            <span>{{ d.label }}</span>
          </div>
        </div>
      </div>
    </div>

    <div class="card panel detail-panel">
      <div class="p-title">用量明细</div>
      <table class="mini-tbl">
        <thead>
          <tr>
            <th>时间</th><th>课堂</th><th>环节</th><th>模型</th><th>用量</th><th>金额</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="u in records.slice(0, 100)" :key="u.id">
            <td>{{ detailTime(u.ts) }}</td>
            <td>{{ sessionName(u.sessionId) }}</td>
            <td>{{ stageName(u.stage) }}</td>
            <td>{{ u.model }}</td>
            <td class="muted">{{ usageDetail(u) }}</td>
            <td>{{ u.amount > 0 ? `¥${u.amount.toFixed(3)}` : "免费" }}</td>
          </tr>
        </tbody>
      </table>
      <div v-if="records.length === 0" class="mini-empty">当前区间暂无用量明细</div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useDataStore } from "../stores/data";
import { useAppStore } from "../stores/app";
import { TIER_OPTIONS } from "../utils/tiers";
import { toDateKey } from "../utils/date";
import { budgetNotifier } from "../utils/notify";
import type { ModelTier, UsageRecord } from "../types";

const store = useDataStore();
const appStore = useAppStore();

const ranges = [
  { id: "today", label: "今日" },
  { id: "week", label: "本周" },
  { id: "month", label: "本月" },
  { id: "all", label: "全部" },
] as const;
const range = ref<(typeof ranges)[number]["id"]>("month");

function inRange(iso: string): boolean {
  const d = new Date(iso);
  const now = new Date();
  if (range.value === "today") return toDateKey(d) === toDateKey(now);
  if (range.value === "all") return true;
  if (range.value === "month")
    return d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth();
  // 本周
  const wd = now.getDay() === 0 ? 7 : now.getDay();
  const monday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - (wd - 1));
  return d >= monday;
}

const records = computed(() =>
  store.usage.filter((u) => inRange(u.ts)).sort((a, b) => b.ts.localeCompare(a.ts))
);

const totalCost = computed(() => records.value.reduce((s, u) => s + u.amount, 0));
const sessionIds = computed(() => new Set(records.value.map((u) => u.sessionId).filter(Boolean)));
const sessionCount = computed(() => sessionIds.value.size);

const sessionsInRange = computed(() =>
  store.sessions.filter((s) => sessionIds.value.has(s.id))
);
const hours = computed(() =>
  sessionsInRange.value.reduce((sum, s) => sum + (s.durationSec ?? 0) / 3600, 0)
);
const asrHours = computed(() =>
  records.value.reduce((s, u) => s + u.audioSeconds / 3600, 0)
);

const freeCount = computed(() => records.value.filter((u) => u.amount === 0).length);
const freePct = computed(() =>
  records.value.length ? Math.round((freeCount.value / records.value.length) * 100) : 0
);
const totalTokens = computed(() =>
  records.value.reduce((s, u) => s + u.inputTokens + u.cachedTokens + u.outputTokens, 0)
);
const totalCached = computed(() =>
  records.value.reduce((s, u) => s + u.cachedTokens, 0)
);
function tokenText(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(0)}K`;
  return String(n);
}
const tokenTextVal = computed(() => tokenText(totalTokens.value));
const cachedText = computed(() => tokenText(totalCached.value));

const budget = computed(() => appStore.settings?.monthlyBudget ?? 30);
const budgetPct = computed(() => Math.min(999, Math.round((totalCost.value / budget.value) * 100)));

// 预算通知：80% / 100% 阈值各触发一次
watch(budgetPct, (p) => {
  if (range.value === "month") budgetNotifier.check(p);
}, { immediate: true });

const STAGE_LABEL: Record<string, string> = {
  asr: "ASR 转写",
  translate: "字幕翻译",
  detect: "行动项检测",
  note: "笔记生成",
  ocr: "课件 OCR",
};
const byStage = computed(() => {
  const map = new Map<string, number>();
  for (const u of records.value) map.set(u.stage, (map.get(u.stage) ?? 0) + u.amount);
  const rows = [...map.entries()].map(([key, amount]) => ({
    key,
    label: STAGE_LABEL[key] ?? key,
    amount,
    pct: 0,
  }));
  const max = Math.max(1, ...rows.map((r) => r.amount));
  rows.forEach((r) => (r.pct = (r.amount / max) * 100));
  return rows.sort((a, b) => b.amount - a.amount);
});

const byTier = computed(() => {
  const map = new Map<string, number>();
  for (const u of records.value) {
    const key = u.modelTier === "local" ? "local" : u.modelTier;
    map.set(key, (map.get(key) ?? 0) + u.amount);
  }
  const rows = TIER_OPTIONS.map((t) => ({
    key: t.id,
    label: t.name,
    emoji: t.emoji,
    amount: map.get(t.id) ?? 0,
    pct: 0,
  }));
  const max = Math.max(1, ...rows.map((r) => r.amount));
  rows.forEach((r) => (r.pct = (r.amount / max) * 100));
  return rows.filter((r) => r.amount > 0 || totalCost.value === 0);
});

const savedByCache = computed(() =>
  +(totalCached.value * 0.0000014 * 0.6).toFixed(2)
);

interface CourseRow {
  courseId: number | null;
  name: string;
  count: number;
  hours: number;
  amount: number;
}
const byCourse = computed<CourseRow[]>(() => {
  const map = new Map<number | null, CourseRow>();
  for (const s of sessionsInRange.value) {
    const key = s.courseId;
    if (!map.has(key)) {
      map.set(key, {
        courseId: key,
        name: store.courseById(key)?.name ?? "未关联课程",
        count: 0,
        hours: 0,
        amount: 0,
      });
    }
    const row = map.get(key)!;
    row.count++;
    row.hours += (s.durationSec ?? 0) / 3600;
    row.amount += s.costEstimate;
  }
  return [...map.values()].sort((a, b) => b.amount - a.amount);
});

const trend7 = computed(() => {
  const now = new Date();
  return Array.from({ length: 7 }, (_, i) => {
    const d = new Date(now.getFullYear(), now.getMonth(), now.getDate() - (6 - i));
    const key = toDateKey(d);
    const amount = store.usage
      .filter((u) => toDateKey(new Date(u.ts)) === key)
      .reduce((s, u) => s + u.amount, 0);
    return {
      key,
      amount,
      isToday: key === toDateKey(now),
      label: `${d.getMonth() + 1}-${d.getDate()}`,
    };
  });
});
const trendMax = computed(() => Math.max(1, ...trend7.value.map((d) => d.amount)));

function sessionName(id: number | null): string {
  const s = store.sessions.find((x) => x.id === id);
  if (!s) return "—";
  const c = store.courseById(s.courseId);
  return `${c?.name ?? "自由记录"} · 第${s.lectureNo}讲`;
}
function detailTime(iso: string): string {
  const d = new Date(iso);
  return `${d.getMonth() + 1}-${String(d.getDate()).padStart(2, "0")} ${String(
    d.getHours()
  ).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
function stageName(s: string): string {
  return STAGE_LABEL[s] ?? s;
}
function usageDetail(u: UsageRecord): string {
  if (u.audioSeconds > 0) return `${u.audioSeconds} 秒音频`;
  const parts: string[] = [];
  if (u.inputTokens) parts.push(`入 ${u.inputTokens}`);
  if (u.cachedTokens) parts.push(`缓存 ${u.cachedTokens}`);
  if (u.outputTokens) parts.push(`出 ${u.outputTokens}`);
  return parts.join(" / ") || "—";
}

void (null as unknown as ModelTier);
</script>

<style scoped>
.cost-page {
  max-width: none;
  /* 最小宽度：花费条目行过窄时改由外层出横向滚动条 */
  min-width: 760px;
}
.range-switch {
  display: flex;
  background: #eef1f7;
  border-radius: 9px;
  padding: 3px;
}
.rs-btn {
  border: none;
  background: transparent;
  padding: 6px 14px;
  border-radius: 7px;
  font-size: 12px;
  cursor: pointer;
  color: var(--ink-soft);
}
.rs-btn.active {
  background: #fff;
  color: var(--brand);
  font-weight: 600;
  box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
}
.kpis {
  display: grid;
  grid-template-columns: repeat(4, 1fr);
  gap: 14px;
  margin-bottom: 16px;
}
.kpi {
  padding: 16px 18px;
}
.kpi-label {
  font-size: 12px;
  color: var(--ink-mute);
}
.kpi-val {
  font-size: 24px;
  font-weight: 700;
  margin: 3px 0;
}
.kpi-val small {
  font-size: 12px;
  font-weight: 400;
  color: var(--ink-mute);
}
.kpi-sub {
  font-size: 11px;
  color: var(--ink-mute);
}
.budget {
  height: 5px;
  background: #e8ecf1;
  border-radius: 3px;
  margin-top: 8px;
  overflow: hidden;
}
.budget i {
  display: block;
  height: 100%;
  background: linear-gradient(90deg, var(--brand), #7c5cff);
}
.budget i.over {
  background: var(--danger);
}
.panels {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 14px;
  margin-bottom: 16px;
}
.panel {
  padding: 16px 20px;
}
.p-title {
  font-size: 13px;
  font-weight: 600;
  margin-bottom: 10px;
}
.p-note {
  font-size: 11px;
  color: var(--ink-mute);
  margin-top: 8px;
}
.bar-row {
  display: grid;
  grid-template-columns: 130px 1fr 56px;
  align-items: center;
  gap: 10px;
  font-size: 12px;
  margin: 8px 0;
}
.bar-row b {
  text-align: right;
}
.bar {
  height: 8px;
  background: #eef1f7;
  border-radius: 4px;
  overflow: hidden;
}
.bar i {
  display: block;
  height: 100%;
  background: var(--brand);
}
.bar.pro i {
  background: #7c5cff;
}
.bar.eco i {
  background: var(--accent);
}
.bar.free i {
  background: #c9d1de;
}
.mini-tbl {
  width: 100%;
  font-size: 12px;
  border-collapse: collapse;
  box-shadow: none;
}
.mini-tbl th {
  text-align: left;
  background: #f4f6fb;
  padding: 7px 9px;
  font-size: 11px;
}
.mini-tbl td {
  padding: 7px 9px;
  border-bottom: 1px solid var(--line);
}
.mini-empty {
  color: var(--ink-mute);
  text-align: center;
  padding: 16px;
}
.muted {
  color: var(--ink-mute);
}
.trend {
  display: flex;
  align-items: flex-end;
  gap: 8px;
  height: 130px;
}
.t-col {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-end;
  gap: 5px;
  height: 100%;
}
.t-col i {
  width: 70%;
  background: #c7d2fe;
  border-radius: 4px 4px 0 0;
}
.t-col.today i {
  background: var(--brand);
}
.t-col b {
  font-size: 10px;
  color: var(--ink-soft);
}
.t-col span {
  font-size: 10px;
  color: var(--ink-mute);
}
.detail-panel {
  margin-bottom: 30px;
}
</style>
