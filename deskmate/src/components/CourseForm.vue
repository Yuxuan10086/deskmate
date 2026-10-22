<template>
  <Teleport to="body">
    <div v-if="show" class="mask" @click.self="close">
      <div class="dialog card">
        <div class="dlg-head">
          <h2>{{ isEdit ? "编辑课程" : "新建课程" }}</h2>
          <button class="dlg-x" @click="close">×</button>
        </div>
        <div class="dlg-body">
          <div class="row-2">
            <div class="field">
              <label>课程名<span class="req">*</span></label>
              <input v-model="form.name" class="input" placeholder="如 学术英语写作" maxlength="40" />
            </div>
            <div class="field">
              <label>课程代码</label>
              <input v-model="form.code" class="input" placeholder="如 ENGL-102" maxlength="20" />
            </div>
          </div>
          <div class="row-2">
            <div class="field">
              <label>上课地点</label>
              <input v-model="form.location" class="input" placeholder="如 文科楼 305" maxlength="40" />
            </div>
            <div class="field">
              <label>起止周<span class="req">*</span></label>
              <div class="week-range">
                <select v-model.number="form.startWeek" class="select">
                  <option v-for="w in 30" :key="w" :value="w">第 {{ w }} 周</option>
                </select>
                <span>至</span>
                <select v-model.number="form.endWeek" class="select">
                  <option v-for="w in 30" :key="w" :value="w">第 {{ w }} 周</option>
                </select>
              </div>
            </div>
          </div>

          <div class="field">
            <label>上课时间<span class="req">*</span>（一周可有多段）</label>
            <div v-for="(slot, i) in form.slots" :key="i" class="slot-row">
              <select v-model.number="slot.weekday" class="select slot-day">
                <option v-for="n in 7" :key="n" :value="n">{{ WEEKDAY_NAMES[n] }}</option>
              </select>
              <input v-model="slot.start" type="time" class="input slot-time" />
              <span class="dash">—</span>
              <input v-model="slot.end" type="time" class="input slot-time" />
              <button
                v-if="form.slots.length > 1"
                class="slot-del"
                title="删除该时段"
                @click="removeSlot(i)"
              >
                ×
              </button>
            </div>
            <button class="btn ghost sm" @click="addSlot">＋ 添加时段</button>
          </div>

          <div class="field">
            <label>课程颜色</label>
            <div class="colors">
              <span
                v-for="c in COURSE_COLORS"
                :key="c"
                class="color-dot"
                :class="{ active: form.color === c }"
                :style="{ background: c }"
                @click="form.color = c"
              ></span>
            </div>
          </div>

          <div v-if="error" class="form-error">{{ error }}</div>
        </div>
        <div class="dlg-foot">
          <button class="btn ghost" @click="close">取消</button>
          <button class="btn" @click="save">保存</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from "vue";
import type { Course, CourseInput, CourseTimeSlot } from "../types";
import { COURSE_COLORS, WEEKDAY_NAMES, hmToMinutes, minutesToHM } from "../utils/date";
import { useDataStore } from "../stores/data";

const props = defineProps<{ show: boolean; course?: Course | null }>();
const emit = defineEmits<{ close: []; saved: [Course] }>();

const dataStore = useDataStore();

interface SlotForm {
  weekday: number;
  start: string;
  end: string;
}
interface FormState {
  name: string;
  code: string;
  location: string;
  startWeek: number;
  endWeek: number;
  color: string;
  slots: SlotForm[];
}

const emptyForm = (): FormState => ({
  name: "",
  code: "",
  location: "",
  startWeek: 1,
  endWeek: 16,
  color: COURSE_COLORS[0],
  slots: [{ weekday: 1, start: "08:00", end: "09:35" }],
});

const form = reactive<FormState>(emptyForm());
const error = ref("");

const isEdit = computed(() => props.course != null);

watch(
  () => props.show,
  (show) => {
    if (!show) return;
    error.value = "";
    if (props.course) {
      const c = props.course;
      Object.assign(form, {
        name: c.name,
        code: c.code ?? "",
        location: c.location ?? "",
        startWeek: c.startWeek,
        endWeek: c.endWeek,
        color: c.color,
        slots:
          c.slots.length > 0
            ? c.slots.map((s) => ({
                weekday: s.weekday,
                start: minutesToHM(s.startMinute),
                end: minutesToHM(s.endMinute),
              }))
            : [{ weekday: 1, start: "08:00", end: "09:35" }],
      });
    } else {
      Object.assign(form, emptyForm());
    }
  }
);

function addSlot() {
  form.slots.push({ weekday: 1, start: "10:00", end: "11:35" });
}
function removeSlot(i: number) {
  form.slots.splice(i, 1);
}
function close() {
  emit("close");
}

async function save() {
  error.value = "";
  if (!form.name.trim()) {
    error.value = "请填写课程名";
    return;
  }
  if (form.endWeek < form.startWeek) {
    error.value = "结束周不能早于开始周";
    return;
  }
  for (const s of form.slots) {
    if (!s.start || !s.end || hmToMinutes(s.end) <= hmToMinutes(s.start)) {
      error.value = `${WEEKDAY_NAMES[s.weekday]} 的时段不合法（结束时间需晚于开始时间）`;
      return;
    }
  }

  const slots: CourseTimeSlot[] = form.slots.map((s) => ({
    weekday: s.weekday,
    startMinute: hmToMinutes(s.start),
    endMinute: hmToMinutes(s.end),
  }));

  const input: CourseInput = {
    name: form.name.trim(),
    code: form.code.trim() || null,
    location: form.location.trim() || null,
    startWeek: form.startWeek,
    endWeek: form.endWeek,
    color: form.color,
    slots,
  };

  try {
    const saved = props.course
      ? ((await dataStore.updateCourse(props.course.id, input)), props.course)
      : await dataStore.addCourse(input);
    emit("saved", saved);
  } catch (e) {
    error.value = String(e);
  }
}
</script>

<style scoped>
.mask {
  position: fixed;
  inset: 0;
  background: rgba(20, 24, 40, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 100;
}
.dialog {
  width: 560px;
  max-height: 88vh;
  display: flex;
  flex-direction: column;
}
.dlg-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 18px 22px 10px;
}
.dlg-head h2 {
  margin: 0;
  font-size: 17px;
}
.dlg-x {
  border: none;
  background: none;
  font-size: 22px;
  color: var(--ink-mute);
  cursor: pointer;
  line-height: 1;
}
.dlg-body {
  padding: 6px 22px 10px;
  overflow-y: auto;
}
.dlg-foot {
  padding: 14px 22px 18px;
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}
.row-2 {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0 16px;
}
.week-range {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--ink-mute);
  font-size: 13px;
}
.slot-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.slot-day {
  width: 92px;
  flex-shrink: 0;
}
.slot-time {
  width: 110px;
  flex-shrink: 0;
}
.dash {
  color: var(--ink-mute);
}
.slot-del {
  border: none;
  background: #fdecec;
  color: var(--danger);
  width: 26px;
  height: 26px;
  border-radius: 6px;
  cursor: pointer;
  font-size: 15px;
}
.colors {
  display: flex;
  gap: 10px;
}
.color-dot {
  width: 24px;
  height: 24px;
  border-radius: 7px;
  cursor: pointer;
  border: 2px solid transparent;
  transition: transform 0.1s;
}
.color-dot:hover {
  transform: scale(1.12);
}
.color-dot.active {
  border-color: #fff;
  box-shadow: 0 0 0 2px var(--ink);
}
.form-error {
  color: var(--danger);
  font-size: 13px;
  margin: 4px 0 8px;
}
</style>
