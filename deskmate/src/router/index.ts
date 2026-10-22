import { createRouter, createWebHistory } from "vue-router";

const routes = [
  { path: "/", redirect: "/courses" },
  { path: "/courses", name: "courses", component: () => import("../views/CoursesView.vue") },
  {
    path: "/courses/:id",
    name: "course-detail",
    component: () => import("../views/CourseDetailView.vue"),
    props: true,
  },
  {
    path: "/sessions/:id",
    name: "session-detail",
    component: () => import("../views/SessionDetailView.vue"),
    props: true,
  },
  { path: "/schedule", name: "schedule", component: () => import("../views/ScheduleView.vue") },
  {
    path: "/session/new",
    name: "new-session",
    component: () => import("../views/NewSessionView.vue"),
  },
  { path: "/tasks", name: "tasks", component: () => import("../views/TasksView.vue") },
  { path: "/cost", name: "cost", component: () => import("../views/CostView.vue") },
  { path: "/settings", name: "settings", component: () => import("../views/SettingsView.vue") },
  { path: "/llm-logs", name: "llm-logs", component: () => import("../views/LlmLogView.vue") },
];

export const router = createRouter({
  history: createWebHistory(),
  routes,
});
