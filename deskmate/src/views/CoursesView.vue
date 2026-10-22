<template>
  <div class="page courses-page">
    <div class="page-head">
      <h1>课程</h1>
      <span class="sub">共 {{ store.courses.length }} 门课</span>
      <div class="spacer"></div>
      <button class="btn" @click="showForm = true">＋ 新建课程</button>
    </div>

    <div v-if="store.courses.length === 0" class="card empty">
      <div class="icon">📚</div>
      <div>还没有课程，点击右上角"新建课程"添加你的第一门课吧</div>
    </div>

    <div v-else class="course-grid">
      <div
        v-for="c in store.courses"
        :key="c.id"
        class="course-card card"
        role="button"
        tabindex="0"
        @click="openCourse(c.id)"
        @keyup.enter="openCourse(c.id)"
      >
        <div class="cc-bar" :style="{ background: c.color }"></div>
        <div class="cc-body">
          <div class="cc-title">{{ c.name }}</div>
          <div class="cc-code">{{ c.code || "—" }}</div>
          <div class="cc-meta">
            <span v-for="(line, i) in slotLines(c)" :key="i">
              <span class="cc-dot" :style="{ background: c.color }"></span>{{ line }}
            </span>
          </div>
          <div class="cc-foot">
            <span class="badge gray">第 {{ c.startWeek }}–{{ c.endWeek }} 周</span>
            <span class="cc-count">{{ store.sessionsOfCourse(c.id).length }} 节课堂</span>
          </div>
        </div>
      </div>
    </div>

    <CourseForm :show="showForm" @close="showForm = false" @saved="onSaved" />
  </div>
</template>

<script setup lang="ts">
import { ref } from "vue";
import { useRouter } from "vue-router";
import { useDataStore } from "../stores/data";
import CourseForm from "../components/CourseForm.vue";
import { WEEKDAY_NAMES, minutesToHM } from "../utils/date";
import type { Course } from "../types";

const router = useRouter();
const store = useDataStore();
const showForm = ref(false);

function slotLines(c: Course): string[] {
  return c.slots.map((s) => {
    const loc = c.location ? ` · ${c.location}` : "";
    return `${WEEKDAY_NAMES[s.weekday]} ${minutesToHM(s.startMinute)}–${minutesToHM(s.endMinute)}${loc}`;
  });
}

function openCourse(id: number) {
  router.push(`/courses/${id}`);
}

function onSaved() {
  showForm.value = false;
}
</script>

<style scoped>
.courses-page {
  /* 最小宽度：只保留单列卡片即可（内容区 < 2×280+16 时网格自动退化为单列）；
     更窄时由外层出横向滚动条，不会把卡片压扁 */
  min-width: 400px;
}
.course-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 16px;
}
.course-card {
  display: flex;
  overflow: hidden;
  cursor: pointer;
  transition: transform 0.15s, box-shadow 0.15s;
}
.course-card:hover {
  transform: translateY(-2px);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.06), 0 12px 32px rgba(0, 0, 0, 0.08);
}
.cc-bar {
  width: 6px;
  flex-shrink: 0;
}
.cc-body {
  padding: 16px 18px;
  flex: 1;
  min-width: 0;
}
.cc-title {
  font-size: 16px;
  font-weight: 700;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cc-code {
  font-size: 12px;
  color: var(--ink-mute);
  margin: 2px 0 10px;
}
.cc-meta {
  display: flex;
  flex-direction: column;
  gap: 4px;
  font-size: 12px;
  color: var(--ink-soft);
  margin-bottom: 12px;
}
.cc-meta > span {
  display: flex;
  align-items: center;
  gap: 6px;
}
.cc-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}
.cc-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.cc-count {
  font-size: 12px;
  color: var(--ink-mute);
}
</style>
