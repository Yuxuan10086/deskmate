<template>
  <div class="page course-page" v-if="course">
    <div class="page-head">
      <button class="btn ghost sm" @click="goBack">← 返回</button>
      <h1>{{ course.name }}</h1>
      <span class="badge blue">{{ course.code || "无代码" }}</span>
      <div class="spacer"></div>
      <button class="btn ghost" @click="showForm = true">编辑课程</button>
      <button class="btn ghost danger-ghost" @click="remove">删除</button>
    </div>

    <div class="info-bar card">
      <div class="info-item" v-for="(line, i) in slotLines" :key="i">
        <span class="info-icon">🕘</span>{{ line }}
      </div>
      <div class="info-item"><span class="info-icon">📌</span>{{ course.location || "未填地点" }}</div>
      <div class="info-item"><span class="info-icon">📆</span>第 {{ course.startWeek }}–{{ course.endWeek }} 周</div>
    </div>

    <div class="sec-head">
      <h2>课堂（{{ sessions.length }}）</h2>
      <div class="sec-actions">
        <button class="btn ghost" @click="triggerUpload">⬆ 上传附件</button>
        <button class="btn" @click="newSession">＋ 新建课堂</button>
        <input ref="fileInput" type="file" multiple hidden @change="onFilesPicked" />
      </div>
    </div>

    <!-- 附件分类选择：上传前逐个指定 课件PPT / 作业题目 / 其他附件 -->
    <div v-if="pendingUploads.length" class="popup-mask">
      <div class="popup-card upload-card">
        <div class="popup-content">这些文件归类为？（将作为课程公共附件）</div>
        <div class="up-list">
          <div v-for="(u, i) in pendingUploads" :key="i" class="up-row">
            <span class="up-name" :title="u.file.name">📄 {{ u.file.name }}</span>
            <select v-model="u.category" class="up-cat">
              <option value="slides">课件 PPT</option>
              <option value="homework">作业题目</option>
              <option value="other">其他附件</option>
            </select>
          </div>
        </div>
        <div class="popup-actions">
          <button class="btn ghost sm" :disabled="uploading" @click="cancelUpload">取消</button>
          <button class="btn sm" :disabled="uploading" @click="confirmUpload">
            {{ uploading ? `上传中 ${uploadDone}/${pendingUploads.length}` : `上传 ${pendingUploads.length} 个文件` }}
          </button>
        </div>
      </div>
    </div>

    <div v-if="sessions.length === 0" class="card empty">
      <div class="icon">🎧</div>
      <div>这门课还没有课堂记录，点击"新建课堂"开始记录第一节</div>
    </div>

    <div v-else class="session-grid">
      <div
        v-for="s in sessions"
        :key="s.id"
        class="session-card card"
        role="button"
        tabindex="0"
        @click="openSession(s.id)"
        @keyup.enter="openSession(s.id)"
      >
        <SessionCover
          :course-name="course.name"
          :color="course.color"
          :lecture-no="s.lectureNo"
          :date-text="coverDate(s.startAt)"
          :start-minute="s.startMinute"
          :end-minute="s.endMinute"
          :mode="s.mode"
        />
        <div class="sc-info">
          <div class="sc-title">
            {{ s.title }}
            <span v-if="s.status === 'recording'" class="badge orange rec-badge">● 录制中</span>
          </div>
          <div class="sc-meta">
            <span>{{ fullDateTime(s.startAt) }}</span>
            <span v-if="s.durationSec">· {{ fmtDur(s.durationSec) }}</span>
            <span v-if="s.costEstimate > 0">· ¥{{ s.costEstimate.toFixed(2) }}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 课程附件：按分类分组，标注所属课堂（课程公共附件不标课堂） -->
    <div class="sec-head mat-head">
      <h2>附件（{{ materials.length }}）</h2>
      <div class="sec-actions">
        <button class="btn ghost sm" @click="triggerUpload">⬆ 上传附件</button>
      </div>
    </div>
    <div v-if="materials.length === 0" class="card empty mat-empty">
      还没有附件。支持任意类型文件，上传时选择「课件 PPT / 作业题目 / 其他附件」。
    </div>
    <div v-else class="card mat-card">
      <template v-for="g in materialGroups" :key="g.key">
        <div v-if="g.items.length" class="mat-group">
          <div class="mat-group-title">{{ g.label }}（{{ g.items.length }}）</div>
          <div v-for="m in g.items" :key="m.id" class="mat-row">
            <span>📄</span>
            <span class="mat-name" :title="m.filePath">{{ m.fileName }}</span>
            <span v-if="m.sessionId != null" class="mat-session">{{ sessionTag(m.sessionId) }}</span>
            <span v-else class="mat-session public">课程公共</span>
            <span class="mat-size">{{ formatSize(m.sizeBytes) }}</span>
            <button class="btn ghost sm" @click="openMaterial(m.id)">打开</button>
            <button class="btn ghost sm danger-text" @click="removeMaterial(m.id)">删除</button>
          </div>
        </div>
      </template>
    </div>

    <transition name="toast">
      <div v-if="toast" class="toast">{{ toast }}</div>
    </transition>

    <CourseForm :show="showForm" :course="course" @close="showForm = false" @saved="onSaved" />
  </div>
  <div v-else class="page empty">课程不存在或已删除。</div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useRouter } from "vue-router";
import { useDataStore } from "../stores/data";
import CourseForm from "../components/CourseForm.vue";
import SessionCover from "../components/SessionCover.vue";
import { WEEKDAY_NAMES, minutesToHM, formatDuration } from "../utils/date";
import type { Material, MaterialCategory } from "../types";
import { api, isTauri, errMsg } from "../api/ipc";

const props = defineProps<{ id: string }>();
const router = useRouter();
const store = useDataStore();

const course = computed(() => store.courseById(Number(props.id)));
const sessions = computed(() =>
  course.value ? store.sessionsOfCourse(course.value.id) : []
);

const showForm = ref(false);
const fileInput = ref<HTMLInputElement | null>(null);
const toast = ref("");
let toastTimer: ReturnType<typeof setTimeout> | undefined;

/* ------------------------------- 课程附件 ------------------------------- */
const materials = ref<Material[]>([]);
const pendingUploads = ref<{ file: File; category: MaterialCategory }[]>([]);
const uploading = ref(false);
const uploadDone = ref(0);

async function loadMaterials() {
  if (!course.value) return;
  try {
    materials.value = await api.listMaterials(course.value.id);
  } catch (e) {
    console.warn("[course] 附件加载失败", e);
  }
}
onMounted(loadMaterials);

const materialGroups = computed(() => {
  const defs: { key: MaterialCategory; label: string }[] = [
    { key: "slides", label: "课件 PPT" },
    { key: "homework", label: "作业题目" },
    { key: "other", label: "其他附件" },
  ];
  return defs.map((d) => ({
    ...d,
    items: materials.value.filter((m) => m.category === d.key),
  }));
});
function sessionTag(sid: number): string {
  const s = store.sessions.find((x) => x.id === sid);
  return s ? s.title : `课堂 #${sid}`;
}
function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}
async function openMaterial(id: number) {
  try {
    const path = await api.openMaterial(id);
    if (!isTauri()) alert(`浏览器环境无法打开本地文件。\n路径：${path}`);
  } catch (e) {
    alert(`打开失败：${errMsg(e)}`);
  }
}
async function removeMaterial(id: number) {
  if (!confirm("确定删除该附件？文件会从本机目录一并移除。")) return;
  try {
    await api.deleteMaterial(id);
    await loadMaterials();
  } catch (e) {
    alert(`删除失败：${errMsg(e)}`);
  }
}

function showToast(msg: string) {
  toast.value = msg;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toast.value = ""), 3600);
}

const slotLines = computed(() =>
  (course.value?.slots ?? []).map(
    (s) =>
      `${WEEKDAY_NAMES[s.weekday]} ${minutesToHM(s.startMinute)}–${minutesToHM(s.endMinute)}`
  )
);

function coverDate(iso: string): string {
  const d = new Date(iso);
  return `${d.getMonth() + 1}/${String(d.getDate()).padStart(2, "0")}`;
}
function fullDateTime(iso: string): string {
  const d = new Date(iso);
  return `${WEEKDAY_NAMES[d.getDay() === 0 ? 7 : d.getDay()]} ${d.getMonth() + 1}月${d.getDate()}日`;
}
function fmtDur(sec: number): string {
  return formatDuration(sec);
}

function goBack() {
  router.push("/courses");
}
function openSession(id: number) {
  router.push(`/sessions/${id}`);
}
function newSession() {
  router.push({ path: "/session/new", query: { courseId: props.id } });
}
function onSaved() {
  showForm.value = false;
  store.reloadCourses();
}
async function remove() {
  if (!course.value) return;
  const msg = ["确定删除课程「", course.value.name, "」及其全部课堂记录吗？此操作不可恢复。"].join("");
  if (!window.confirm(msg)) return;
  await store.removeCourse(course.value.id);
  router.push("/courses");
}
function triggerUpload() {
  fileInput.value?.click();
}
/** 演示/文档类默认归为课件 PPT，其余默认其他附件。 */
function guessCategory(name: string): MaterialCategory {
  return /\.(pptx?|pdf|key|keyx)$/i.test(name) ? "slides" : "other";
}
function onFilesPicked(e: Event) {
  const input = e.target as HTMLInputElement;
  if (!input.files || input.files.length === 0) return;
  for (const f of Array.from(input.files)) {
    pendingUploads.value.push({ file: f, category: guessCategory(f.name) });
  }
  input.value = "";
}
function cancelUpload() {
  if (uploading.value) return;
  pendingUploads.value = [];
}
function fileToBase64(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(",").pop() ?? "");
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}
async function confirmUpload() {
  if (!course.value || uploading.value) return;
  uploading.value = true;
  uploadDone.value = 0;
  let ok = 0;
  for (const u of pendingUploads.value) {
    try {
      const b64 = await fileToBase64(u.file);
      // 课程级附件：不带 sessionId，作为课程公共资料
      await api.uploadMaterial({
        fileName: u.file.name,
        base64: b64,
        courseId: course.value.id,
        category: u.category,
      });
      ok++;
    } catch (err) {
      console.error("上传失败", u.file.name, err);
    }
    uploadDone.value++;
  }
  pendingUploads.value = [];
  uploading.value = false;
  await loadMaterials();
  showToast(`已上传 ${ok} 个附件至课程公共资料`);
}
</script>

<style scoped>
.course-page {
  /* 最小宽度：课堂卡片/附件行过窄时改由外层出横向滚动条；不能太宽，否则窄窗口+右侧聊天栏时大片空白 */
  min-width: 560px;
}
.danger-ghost {
  color: var(--danger);
}
.danger-ghost:hover {
  border-color: var(--danger);
  background: #fef2f2;
}
.info-bar {
  display: flex;
  flex-wrap: wrap;
  gap: 24px;
  padding: 14px 20px;
  margin-bottom: 26px;
  font-size: 13px;
  color: var(--ink-soft);
}
.info-icon {
  margin-right: 6px;
}
.sec-head {
  display: flex;
  align-items: center;
  margin-bottom: 14px;
}
.sec-head h2 {
  font-size: 15px;
  margin: 0;
  font-weight: 600;
}
.sec-actions {
  margin-left: auto;
  display: flex;
  gap: 10px;
}
.session-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 16px;
}
.session-card {
  padding: 10px;
  cursor: pointer;
  transition: transform 0.15s, box-shadow 0.15s;
}
.session-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.06), 0 12px 32px rgba(0, 0, 0, 0.08);
}
.sc-info {
  padding: 8px 4px 4px;
}
.sc-title {
  font-size: 13px;
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 8px;
}
.rec-badge {
  font-weight: 500;
}
.sc-meta {
  font-size: 11px;
  color: var(--ink-mute);
  margin-top: 3px;
  display: flex;
  gap: 4px;
}
.toast {
  position: fixed;
  bottom: 28px;
  left: 50%;
  transform: translateX(-50%);
  background: rgba(26, 26, 46, 0.92);
  color: #fff;
  padding: 10px 20px;
  border-radius: 10px;
  font-size: 13px;
  z-index: 200;
}
.toast-enter-active,
.toast-leave-active {
  transition: opacity 0.25s, transform 0.25s;
}
.toast-enter-from,
.toast-leave-to {
  opacity: 0;
  transform: translateX(-50%) translateY(8px);
}

/* 附件 */
.mat-head {
  margin-top: 26px;
}
.mat-empty {
  padding: 24px;
  text-align: center;
  color: var(--ink-mute);
  font-size: 13px;
}
.mat-card {
  padding: 14px 20px;
}
.mat-group {
  margin-bottom: 12px;
}
.mat-group:last-child {
  margin-bottom: 0;
}
.mat-group-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--ink-soft);
  padding: 6px 8px;
  background: #fafbfe;
  border-radius: 6px;
}
.mat-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 4px;
  border-bottom: 1px solid var(--line);
  font-size: 13px;
}
.mat-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mat-session {
  font-size: 11px;
  color: var(--brand);
  background: var(--brand-soft);
  border-radius: 999px;
  padding: 2px 8px;
  flex-shrink: 0;
}
.mat-session.public {
  color: var(--ink-mute);
  background: #f1f3f8;
}
.mat-size {
  font-size: 11px;
  color: var(--ink-mute);
  min-width: 52px;
  text-align: right;
}
.danger-text {
  color: var(--danger);
}

/* 附件分类弹窗 */
.popup-mask {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.35);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 60;
}
.popup-card {
  width: 460px;
  max-width: calc(100vw - 40px);
  background: #fff;
  border-radius: 14px;
  padding: 20px 22px;
  box-shadow: 0 12px 40px rgba(15, 23, 42, 0.25);
}
.popup-content {
  font-size: 14px;
  font-weight: 600;
  line-height: 1.6;
}
.popup-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
.up-list {
  margin: 12px 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: 320px;
  overflow-y: auto;
}
.up-row {
  display: flex;
  align-items: center;
  gap: 10px;
}
.up-name {
  flex: 1;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.up-cat {
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 4px 8px;
  font-size: 12px;
  background: #fff;
  flex-shrink: 0;
}
</style>
