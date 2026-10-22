<template>
  <div class="page narrow">
    <div class="page-head">
      <button class="btn ghost sm" @click="$router.back()">← 返回</button>
      <h1>新建课堂</h1>
    </div>

    <div class="prefill-tip card">
      <span class="tip-icon">✨</span>
      <span>{{ prefillText }}</span>
    </div>

    <div class="form-cols">
      <div class="card form-card">
        <h3 class="fc-title">课堂信息</h3>

        <div class="field">
          <label>所属课程<span class="req">*</span></label>
          <select v-model.number="form.courseId" class="select" @change="onCourseChange">
            <option :value="null">未关联课程（自由记录）</option>
            <option v-for="c in store.courses" :key="c.id" :value="c.id">{{ c.name }}</option>
          </select>
        </div>

        <div class="row-3">
          <div class="field">
            <label>周次</label>
            <input v-model.number="form.weekNo" type="number" min="1" max="30" class="input" />
          </div>
          <div class="field">
            <label>星期</label>
            <select v-model.number="form.weekday" class="select">
              <option v-for="n in 7" :key="n" :value="n">{{ WEEKDAY_NAMES[n] }}</option>
            </select>
          </div>
          <div class="field">
            <label>地点</label>
            <input v-model="form.location" class="input" placeholder="选填" />
          </div>
        </div>

        <div class="field">
          <label>上课时段</label>
          <div class="time-row">
            <input v-model="startHM" type="time" class="input" />
            <span>—</span>
            <input v-model="endHM" type="time" class="input" />
          </div>
        </div>

        <div class="field">
          <label>附件（可选）</label>
          <button class="btn ghost upload-btn" @click="triggerPpt">
            ⬆ 选择附件（支持任意类型，可多选）
          </button>
          <input ref="pptInput" type="file" multiple hidden @change="onPpt" />
          <div v-if="pickedUploads.length" class="picked">
            <div v-for="(u, i) in pickedUploads" :key="i" class="picked-file">
              <span class="pf-name" :title="u.file.name">📄 {{ u.file.name }}</span>
              <span class="pf-size">{{ kb(u.file.size) }}</span>
              <select v-model="u.category" class="pf-cat">
                <option value="slides">课件 PPT</option>
                <option value="homework">作业题目</option>
                <option value="other">其他附件</option>
              </select>
              <button type="button" class="pf-del" @click="removePicked(i)">✕</button>
            </div>
          </div>
          <div class="hint">课件 / 作业题 / 任意资料均可；课后也可在课堂主页补传</div>
        </div>
      </div>

      <div class="card form-card">
        <h3 class="fc-title">
          模型系列
          <!-- 同传模型全档固定（Doubao-AST-2.0），只在此处标注一次 -->
          <span class="fc-note">同传 ≈¥{{ astRate }}<small>/小时</small>（各档相同）</span>
        </h3>
        <div class="tier-list">
          <div
            v-for="t in TIER_OPTIONS"
            :key="t.id"
            class="tier-opt"
            :class="{ active: form.modelTier === t.id }"
            @click="form.modelTier = t.id"
          >
            <span class="t-emoji">{{ t.emoji }}</span>
            <div class="t-main">
              <div class="t-name">
                {{ t.name }}
                <span v-if="t.recommended" class="badge blue">推荐</span>
              </div>
              <div class="t-desc">{{ t.desc }}</div>
            </div>
            <!-- 各档差异在非实时（经济模式）单价 -->
            <div class="t-rate">
              非实时 ≈¥{{ t.chunkRatePerHour }}<small>/小时</small>
            </div>
          </div>
        </div>
      </div>
    </div>

    <div class="card form-card mode-card">
      <h3 class="fc-title">记录模式</h3>
      <div class="mode-grid">
        <div
          v-for="(info, key) in MODE_INFO"
          :key="key"
          class="mode-opt"
          :class="{ active: form.mode === key }"
          @click="form.mode = key as SessionMode"
        >
          <div class="mo-emoji">{{ info.emoji }}</div>
          <div class="mo-name">{{ info.name }}</div>
          <div class="mo-desc">{{ info.desc }}</div>
        </div>
      </div>
    </div>

    <div class="start-bar card">
      <div class="sb-cost">
        预计花费
        <b>同传 ≈ ¥{{ tier.astRatePerHour.toFixed(1) }}/小时</b>
        <b class="sb-cost-sub">非实时 ≈ ¥{{ tier.chunkRatePerHour.toFixed(1) }}/小时</b>
        <span class="sb-note">录制中可随时切换，实际以结束后统计为准</span>
      </div>
      <button class="btn start-btn" :disabled="starting" @click="start">
        {{ starting ? "正在启动…" : "● 开始记录这节课" }}
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useDataStore } from "../stores/data";
import { useAppStore } from "../stores/app";
import { WEEKDAY_NAMES, hmToMinutes, minutesToHM, weekdayOf } from "../utils/date";
import { prefillSession } from "../utils/prefill";
import { MODE_INFO, TIER_OPTIONS } from "../utils/tiers";
import type { MaterialCategory, ModelTier, SessionMode } from "../types";
import { api } from "../api/ipc";

const route = useRoute();
const router = useRouter();
const store = useDataStore();
const appStore = useAppStore();

const form = reactive({
  courseId: null as number | null,
  weekNo: 1,
  weekday: 1,
  location: "",
  mode: "offline" as SessionMode,
  modelTier: "balanced" as ModelTier,
});
const startHM = ref("08:00");
const endHM = ref("09:35");
const pptInput = ref<HTMLInputElement | null>(null);
const pickedUploads = ref<{ file: File; category: MaterialCategory }[]>([]);
const starting = ref(false);
const matchedCourse = ref("");

const tier = computed(() => TIER_OPTIONS.find((t) => t.id === form.modelTier)!);
/** 同传（Doubao-AST-2.0）各档同一单价，页面上只展示一次 */
const astRate = computed(() => TIER_OPTIONS[0].astRatePerHour.toFixed(1));

const prefillText = computed(() => {
  // 从课程表格子跳入：按格子所在周次/星期/节次预填
  if (route.query.weekday != null) {
    const tail = matchedCourse.value ? `，课程：${matchedCourse.value}` : "";
    return `已按课程表预填：第 ${form.weekNo} 周 · ${WEEKDAY_NAMES[form.weekday] || ""} ${startHM.value}–${endHM.value}${tail}`;
  }
  // 直接进入：按当前系统时间匹配
  const now = new Date();
  const pad = (n: number) => String(n).padStart(2, "0");
  const base = `系统时间 ${WEEKDAY_NAMES[weekdayOf(now)] || ""} ${pad(now.getHours())}:${pad(now.getMinutes())} · 第 ${form.weekNo} 周`;
  if (matchedCourse.value) return `${base}，已自动匹配课程：${matchedCourse.value}`;
  return `${base}，未在课程表中匹配到当前时段的课程，可手动选择`;
});

onMounted(() => {
  const p = prefillSession(new Date(), store.courses, appStore.settings!.semesterStart);

  // query 覆盖（从课程表格子跳入）
  const qCourse = route.query.courseId ? Number(route.query.courseId) : undefined;
  const qWeek = route.query.week ? Number(route.query.week) : undefined;
  const qWeekday = route.query.weekday ? Number(route.query.weekday) : undefined;
  const qStart = route.query.start ? String(route.query.start) : undefined;
  const qEnd = route.query.end ? String(route.query.end) : undefined;

  form.courseId = qCourse ?? p.courseId;
  form.weekNo = qWeek ?? p.weekNo;
  form.weekday = qWeekday ?? p.weekday;
  const chosenCourse = form.courseId != null ? store.courseById(form.courseId) : undefined;
  form.location = chosenCourse?.location ?? p.location ?? "";
  startHM.value = qStart ?? minutesToHM(p.startMinute);
  endHM.value = qEnd ?? minutesToHM(p.endMinute);

  if (form.courseId != null) {
    matchedCourse.value = store.courseById(form.courseId)?.name ?? "";
  }
});

function onCourseChange() {
  if (form.courseId != null) {
    const c = store.courseById(form.courseId);
    if (c?.location) form.location = c.location;
  }
}
function triggerPpt() {
  pptInput.value?.click();
}
/** 演示/文档类默认归为课件 PPT，其余默认其他附件。 */
function guessCategory(name: string): MaterialCategory {
  return /\.(pptx?|pdf|key|keyx)$/i.test(name) ? "slides" : "other";
}
function onPpt(e: Event) {
  const files = Array.from((e.target as HTMLInputElement).files ?? []);
  for (const f of files) {
    pickedUploads.value.push({ file: f, category: guessCategory(f.name) });
  }
  (e.target as HTMLInputElement).value = "";
}
function removePicked(i: number) {
  pickedUploads.value.splice(i, 1);
}
function kb(bytes: number): string {
  return bytes > 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : `${Math.round(bytes / 1024)} KB`;
}

function fileToBase64(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(",").pop() ?? "");
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}
async function start() {
  if (!hmToMinutes(endHM.value) || hmToMinutes(endHM.value) <= hmToMinutes(startHM.value)) {
    window.alert("上课时段不合法");
    return;
  }
  starting.value = true;
  try {
    const s = await store.addSession({
      courseId: form.courseId,
      mode: form.mode,
      modelTier: form.modelTier,
      weekNo: form.weekNo,
      weekday: form.weekday,
      startMinute: hmToMinutes(startHM.value),
      endMinute: hmToMinutes(endHM.value),
      location: form.location.trim() || null,
    });
    // 上传预先选中的附件（按用户指定分类落盘）
    if (pickedUploads.value.length > 0) {
      for (const u of pickedUploads.value) {
        try {
          const b64 = await fileToBase64(u.file);
          await api.uploadMaterial({
            fileName: u.file.name,
            base64: b64,
            courseId: form.courseId ?? 0,
            sessionId: s.id,
            category: u.category,
          });
        } catch (err) {
          console.error("附件上传失败", u.file.name, err);
        }
      }
      pickedUploads.value = [];
    }
    router.replace(`/sessions/${s.id}`);
  } finally {
    starting.value = false;
  }
}
</script>

<style scoped>
.narrow {
  max-width: 960px;
  /* 最小宽度：双列表单过窄时改由外层出横向滚动条 */
  min-width: 720px;
}
.prefill-tip {
  display: flex;
  gap: 10px;
  align-items: center;
  padding: 12px 18px;
  margin-bottom: 16px;
  font-size: 13px;
  color: var(--ink-soft);
  background: linear-gradient(90deg, var(--brand-soft), #f6f3ff);
  border-color: #d9e0ff;
}
.tip-icon {
  font-size: 16px;
}
.form-cols {
  display: grid;
  grid-template-columns: 1.05fr 1fr;
  gap: 16px;
  /* 两列卡片等高对齐，避免一列底部出现大片空白 */
  align-items: stretch;
}
.mode-card {
  margin-top: 16px;
}
.form-card {
  padding: 18px 20px;
}
.fc-title {
  margin: 0 0 14px;
  font-size: 14px;
  font-weight: 700;
  display: flex;
  align-items: baseline;
  gap: 10px;
}
.fc-note {
  font-size: 11px;
  font-weight: 500;
  color: var(--ink-mute);
}
.fc-note small {
  font-size: 10px;
}
.row-3 {
  display: grid;
  grid-template-columns: 90px 100px 1fr;
  gap: 12px;
}
.time-row {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--ink-mute);
}
.time-row .input {
  width: 130px;
}
.upload-btn {
  width: 100%;
  padding: 10px;
}
.picked {
  margin-top: 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.picked-file {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  color: var(--ink-soft);
  background: #f6f8fc;
  border-radius: 6px;
  padding: 5px 10px;
}
.pf-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.pf-size {
  color: var(--ink-mute);
  flex-shrink: 0;
}
.pf-cat {
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 2px 6px;
  font-size: 12px;
  background: #fff;
  flex-shrink: 0;
}
.pf-del {
  border: none;
  background: transparent;
  color: var(--ink-mute);
  cursor: pointer;
  font-size: 12px;
  padding: 0 2px;
}
.pf-del:hover {
  color: var(--danger);
}
.mode-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 10px;
}
.mode-opt {
  border: 1.5px solid var(--line);
  border-radius: 10px;
  padding: 14px 12px;
  text-align: center;
  cursor: pointer;
  transition: all 0.15s;
}
.mode-opt:hover {
  border-color: var(--brand);
}
.mode-opt.active {
  border-color: var(--brand);
  background: var(--brand-soft);
  box-shadow: 0 0 0 1px var(--brand);
}
.mo-emoji {
  font-size: 24px;
}
.mo-name {
  font-weight: 600;
  font-size: 13px;
  margin: 4px 0 2px;
}
.mo-desc {
  font-size: 11px;
  color: var(--ink-mute);
  line-height: 1.4;
}
.tier-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.tier-opt {
  display: flex;
  align-items: center;
  gap: 10px;
  border: 1.5px solid var(--line);
  border-radius: 10px;
  padding: 10px 12px;
  cursor: pointer;
  transition: all 0.15s;
}
.tier-opt:hover {
  border-color: var(--brand);
}
.tier-opt.active {
  border-color: var(--brand);
  background: var(--brand-soft);
  box-shadow: 0 0 0 1px var(--brand);
}
.t-emoji {
  font-size: 20px;
}
.t-main {
  flex: 1;
  min-width: 0;
}
.t-name {
  font-size: 13px;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 6px;
}
.t-desc {
  font-size: 11px;
  color: var(--ink-mute);
  margin-top: 2px;
}
.t-rate {
  font-size: 13px;
  font-weight: 700;
  color: var(--brand);
  white-space: nowrap;
  flex-shrink: 0;
}
.t-rate small {
  font-size: 10px;
  font-weight: 400;
  color: var(--ink-mute);
}
.start-bar {
  position: sticky;
  bottom: 16px;
  margin-top: 18px;
  padding: 14px 20px;
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.sb-cost b {
  color: var(--brand);
  font-size: 15px;
}
.sb-cost-sub {
  margin-left: 10px;
  color: var(--ink-mute) !important;
  font-size: 13px !important;
  font-weight: 600;
}
.sb-note {
  font-size: 11px;
  color: var(--ink-mute);
  margin-left: 8px;
}
.start-btn {
  padding: 10px 26px;
  font-size: 14px;
}
</style>
