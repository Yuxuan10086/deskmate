<template>
  <div class="page schedule-page" :style="{ minWidth: minWidth + 'px' }">
    <div class="page-head">
      <h1>课程表</h1>
      <span class="sub">第 {{ weekNo }} 周 · {{ rangeText }}</span>
      <div class="spacer"></div>
      <label class="weekend-toggle">
        <input v-model="showWeekend" type="checkbox" />
        显示周末
      </label>
      <button class="btn ghost sm" :disabled="weekNo <= 1" @click="weekNo--">← 上一周</button>
      <button class="btn ghost sm" @click="goThisWeek">本周</button>
      <button class="btn ghost sm" @click="weekNo++">下一周 →</button>
    </div>

    <div class="card legend-bar">
      <span><i class="lg has"></i> 有课堂记录（点击进入）</span>
      <span><i class="lg rec"></i> 录制中</span>
      <span><i class="lg none"></i> 已排课 · 暂无记录（点击新建课堂）</span>
    </div>

    <div class="grid-wrap card">
      <div
        class="sched-grid"
        :style="{
          '--hour-px': HOUR_PX + 'px',
          gridTemplateColumns: `64px repeat(${weekDates.length}, minmax(96px, 1fr))`,
        }"
      >
        <div class="corner"></div>
        <div
          v-for="(d, i) in weekDates"
          :key="`h${i}`"
          class="day-head"
          :class="{ today: isToday(d) }"
        >
          <div class="wd-name">{{ WEEKDAY_NAMES[i + 1] }}</div>
          <div class="wd-date">{{ d.getMonth() + 1 }}/{{ d.getDate() }}</div>
        </div>

        <div class="time-col">
          <div
            v-for="h in hours"
            :key="h"
            class="hour-label"
            :class="{ last: h === endHour }"
          >
            <span>{{ h === 24 ? "24:00" : `${String(h).padStart(2, "0")}:00` }}</span>
          </div>
        </div>

        <div
          v-for="(d, col) in weekDates"
          :key="`d${col}`"
          class="day-col"
          :class="{ todayCol: isToday(d) }"
        >
          <div v-for="h in slotHours" :key="h" class="hour-line"></div>
          <div v-if="isToday(d) && nowTop !== null" class="now-line" :style="{ top: nowTop + 'px' }">
            <i></i>
          </div>
          <div
            v-for="b in blocksFor(col + 1, d)"
            :key="b.key"
            class="course-block"
            :class="b.session ? 'has' : 'none'"
            role="button"
            tabindex="0"
            :style="blockStyle(b)"
            @click="b.session ? openSession(b.session!.id) : newFor(b)"
            @keyup.enter="b.session ? openSession(b.session!.id) : newFor(b)"
          >
            <div class="cb-time">{{ minutesToHM(b.realStart) }}–{{ minutesToHM(b.realEnd) }}</div>
            <div class="cb-name">{{ b.courseName }}</div>
            <div v-if="b.location" class="cb-loc">{{ b.location }}</div>
            <div v-if="b.session && b.session.status === 'recording'" class="cb-rec">● 录制中</div>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useDataStore } from "../stores/data";
import { useAppStore } from "../stores/app";
import type { ClassSession } from "../types";
import { WEEKDAY_NAMES, datesOfWeek, minutesToHM, toDateKey, weekNumberOf } from "../utils/date";

/** 每个整点小节的像素高度（小节之间无缝隙） */
const HOUR_PX = 56;

const router = useRouter();
const store = useDataStore();
const appStore = useAppStore();

const startHour = computed(() => appStore.settings!.scheduleStartHour);
const endHour = computed(() => appStore.settings!.scheduleEndHour);
/** 需要画横线/时间标签的整点：start … end（end 为底部闭合线） */
const hours = computed(() => {
  const arr: number[] = [];
  for (let h = startHour.value; h <= endHour.value; h++) arr.push(h);
  return arr;
});
/** 实际小节：start … end-1，每节一小时 */
const slotHours = computed(() => {
  const arr: number[] = [];
  for (let h = startHour.value; h < endHour.value; h++) arr.push(h);
  return arr;
});
const gridHeight = computed(() => (endHour.value - startHour.value) * HOUR_PX);

const thisWeek = weekNumberOf(new Date(), appStore.settings!.semesterStart);
const weekNo = ref(thisWeek);
/** 默认只看工作日，勾选后才展开周六周日 */
const showWeekend = ref(false);

const weekDates = computed(() => {
  const all = datesOfWeek(weekNo.value, appStore.settings!.semesterStart);
  return showWeekend.value ? all : all.slice(0, 5);
});
/** 页面最小宽度：时间列 64 + 每天下限 96 + 页面左右留白 64；窄于此由外层出横向滚动条 */
const minWidth = computed(() => 128 + weekDates.value.length * 96);
const rangeText = computed(() => {
  const a = weekDates.value[0];
  const b = weekDates.value[weekDates.value.length - 1];
  return `${a.getMonth() + 1}月${a.getDate()}日 – ${b.getMonth() + 1}月${b.getDate()}日`;
});

/* ------------------------------- 当前时间线 ------------------------------- */

const nowMinute = ref<number | null>(null);
let nowTimer: number | undefined;
function refreshNow() {
  const d = new Date();
  nowMinute.value = d.getHours() * 60 + d.getMinutes();
}
const nowTop = computed(() => {
  if (nowMinute.value === null) return null;
  const top = ((nowMinute.value - startHour.value * 60) / 60) * HOUR_PX;
  return top >= 0 && top <= gridHeight.value ? top : null;
});
onMounted(() => {
  refreshNow();
  nowTimer = window.setInterval(refreshNow, 30_000);
  // 兜底刷新：其他页面删除课堂后回到课程表时确保数据最新（store 过滤已同步，此处双保险）
  void store.reloadSessions().catch(() => undefined);
});
onBeforeUnmount(() => {
  if (nowTimer) window.clearInterval(nowTimer);
});

/* --------------------------------- 数据块 --------------------------------- */

interface Block {
  key: string;
  courseId: number | null;
  courseName: string;
  location: string | null;
  session: ClassSession | null;
  color: string;
  /** 原始起止（分钟），用于展示与新建课堂跳转 */
  realStart: number;
  realEnd: number;
  /** 裁剪到课程表可见范围后的起止（分钟），用于定位 */
  start: number;
  end: number;
  weekday: number;
  /** 同一时段冲突时的横向分栏 */
  lane: number;
  laneCount: number;
}

function isToday(d: Date): boolean {
  return toDateKey(d) === toDateKey(new Date());
}
function goThisWeek() {
  weekNo.value = thisWeek;
}

/**
 * 计算某天列的全部课程块：
 * 1. 当天课堂记录（实际起止）
 * 2. 当周有效、但当天无重叠课堂的已排课 slot（虚线块）
 * 块按开始时间排序后贪心分配横向分栏（lane），解决时间冲突。
 */
function blocksFor(weekday: number, date: Date): Block[] {
  const dateKey = toDateKey(date);
  const sessions = store.sessionsOnDate(dateKey);
  const raw: Block[] = [];

  for (const s of sessions) {
    const c = store.courseById(s.courseId);
    raw.push({
      key: `s${s.id}`,
      courseId: s.courseId,
      courseName: c?.name ?? "未关联课程",
      location: s.location,
      session: s,
      color: c?.color ?? "#64748b",
      realStart: s.startMinute,
      realEnd: s.endMinute,
      start: s.startMinute,
      end: s.endMinute,
      weekday,
      lane: 0,
      laneCount: 1,
    });
  }

  for (const c of store.courses) {
    if (weekNo.value < c.startWeek || weekNo.value > c.endWeek) continue;
    for (const slot of c.slots) {
      if (slot.weekday !== weekday) continue;
      // 与当天任意课堂重叠则不重复展示
      const overlap = sessions.some(
        (s) => s.startMinute < slot.endMinute && slot.startMinute < s.endMinute
      );
      if (overlap) continue;
      raw.push({
        key: `c${c.id}-${slot.startMinute}`,
        courseId: c.id,
        courseName: c.name,
        location: c.location,
        session: null,
        color: c.color,
        realStart: slot.startMinute,
        realEnd: slot.endMinute,
        start: slot.startMinute,
        end: slot.endMinute,
        weekday,
        lane: 0,
        laneCount: 1,
      });
    }
  }

  const rangeStart = startHour.value * 60;
  const rangeEnd = endHour.value * 60;
  const visible = raw
    .filter((b) => b.end > rangeStart && b.start < rangeEnd)
    .map((b) => ({ ...b, start: Math.max(b.start, rangeStart), end: Math.min(b.end, rangeEnd) }));

  visible.sort((a, b) => a.start - b.start || b.end - a.end);

  // 贪心分栏：laneEnd[i] 记录第 i 栏当前块结束时间
  const laneEnd: number[] = [];
  for (const b of visible) {
    let lane = laneEnd.findIndex((end) => end <= b.start);
    if (lane < 0) {
      lane = laneEnd.length;
      laneEnd.push(b.end);
    } else {
      laneEnd[lane] = b.end;
    }
    b.lane = lane;
  }
  // 每块所在时刻的并发栏数，用于宽度/左偏移
  for (const b of visible) {
    b.laneCount = Math.max(
      1,
      ...visible
        .filter((o) => o !== b && o.start < b.end && b.start < o.end)
        .map((o) => o.lane + 1),
      b.lane + 1
    );
  }
  return visible;
}

function blockStyle(b: Block) {
  const top = ((b.start - startHour.value * 60) / 60) * HOUR_PX;
  const height = Math.max(((b.end - b.start) / 60) * HOUR_PX - 2, 18);
  const widthPct = 100 / b.laneCount;
  return {
    top: `${top}px`,
    height: `${height}px`,
    width: `calc(${widthPct}% - 4px)`,
    left: `calc(${widthPct * b.lane}% + 2px)`,
    background: b.session ? b.color : undefined,
    borderColor: b.session ? undefined : b.color,
    color: b.session ? "#fff" : b.color,
  };
}

function openSession(id: number) {
  router.push(`/sessions/${id}`);
}

function newFor(b: Block) {
  router.push({
    path: "/session/new",
    query: {
      courseId: b.courseId != null ? String(b.courseId) : undefined,
      week: String(weekNo.value),
      weekday: String(b.weekday),
      start: minutesToHM(b.realStart),
      end: minutesToHM(b.realEnd),
    },
  });
}
</script>

<style scoped>
.schedule-page {
  max-width: none;
  /* min-width 由脚本按当前列数（5/7 天）动态计算，见 minWidth */
}
.legend-bar {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 24px;
  padding: 10px 18px;
  margin-bottom: 14px;
  font-size: 12px;
  color: var(--ink-soft);
}
.weekend-toggle {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--ink-soft);
  cursor: pointer;
  user-select: none;
}
.weekend-toggle input {
  cursor: pointer;
  margin: 0;
}
.lg {
  display: inline-block;
  width: 14px;
  height: 14px;
  border-radius: 4px;
  vertical-align: -2px;
  margin-right: 6px;
}
.lg.has {
  background: var(--brand);
}
.lg.rec {
  background: var(--warn);
}
.lg.none {
  border: 1.5px dashed var(--line-strong);
}
/* 不再自带滚动：课表高度自然撑开，纵横向滚动统一交给外层 .content */
.grid-wrap {
  padding: 0;
}
.sched-grid {
  display: grid;
  /* 列数随「显示周末」切换（5 或 7 列），实际值由模板内联 gridTemplateColumns 给出 */
  grid-template-columns: 64px repeat(5, minmax(96px, 1fr));
  grid-auto-rows: min-content;
  border-top: 1px solid var(--line);
}
.corner {
  position: sticky;
  left: 32px;
  /* 高于课程块（z2/悬停z4），低于 .time-col（z6，08:00 等标签上探进本格区域） */
  z-index: 5;
  background: #fafbfe;
  border-right: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
}
.day-head {
  text-align: center;
  background: #fafbfe;
  padding: 8px 4px;
  font-weight: 500;
  border-right: 1px solid var(--line);
  border-bottom: 1px solid var(--line);
}
.day-head.today {
  background: var(--brand-soft);
}
.wd-name {
  font-size: 13px;
  color: var(--ink-soft);
}
.wd-date {
  font-size: 11px;
  color: var(--ink-mute);
}
.time-col {
  position: sticky;
  left: 32px;
  /* 全表最高：横向滚动时课程块（z2/悬停z4）从下方滑过不得遮挡时间标签 */
  z-index: 6;
  background: #fafbfe;
  border-right: 1px solid var(--line);
}
.hour-label {
  height: var(--hour-px);
  position: relative;
}
.hour-label.last {
  height: 0;
}
.hour-label span {
  position: absolute;
  top: -7px;
  right: 6px;
  font-size: 10px;
  color: var(--ink-mute);
  background: #fafbfe;
  padding: 0 2px;
}
.day-col {
  position: relative;
  border-right: 1px solid var(--line);
}
.day-col:last-child {
  border-right: none;
}
.day-col.todayCol {
  background: rgba(238, 241, 255, 0.55);
}
.hour-line {
  height: var(--hour-px);
  border-bottom: 1px solid var(--line);
}
.now-line {
  position: absolute;
  left: 0;
  right: 0;
  z-index: 1;
  pointer-events: none;
}
.now-line::before {
  content: "";
  position: absolute;
  left: 0;
  right: 0;
  top: 0;
  border-top: 1.5px solid #e23b3b;
}
.now-line i {
  position: absolute;
  left: -3px;
  top: -4px;
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #e23b3b;
}
.course-block {
  position: absolute;
  z-index: 2;
  border-radius: 6px;
  padding: 3px 7px;
  cursor: pointer;
  overflow: hidden;
  transition: transform 0.1s;
}
.course-block:hover {
  transform: scale(1.015);
  z-index: 4;
}
.course-block.has {
  color: #fff;
}
.course-block.none {
  border: 1.5px dashed;
  background: #fff;
}
.cb-time {
  font-size: 10px;
  opacity: 0.85;
  line-height: 1.4;
}
.course-block.none .cb-time {
  opacity: 0.75;
}
.cb-name {
  font-size: 12px;
  font-weight: 600;
  line-height: 1.35;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cb-loc {
  font-size: 10px;
  opacity: 0.85;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cb-rec {
  font-size: 10px;
  margin-top: 1px;
  opacity: 0.95;
}
</style>
