import { defineStore } from "pinia";
import { api } from "../api/ipc";
import type {
  ClassSession,
  Course,
  CourseInput,
  SessionInput,
  Todo,
  UsageRecord,
} from "../types";
import { toDateKey } from "../utils/date";

export const useDataStore = defineStore("data", {
  state: () =>
    ({
      courses: [] as Course[],
      sessions: [] as ClassSession[],
      usage: [] as UsageRecord[],
      todos: [] as Todo[],
      loading: false,
    }),
  getters: {
    courseById: (state) => (id: number | null) =>
      id == null ? undefined : state.courses.find((c) => c.id === id),
    sessionsOfCourse: (state) => (courseId: number) =>
      state.sessions
        .filter((s) => s.courseId === courseId)
        .sort((a, b) => b.startAt.localeCompare(a.startAt)),
  },
  actions: {
    async loadAll() {
      this.loading = true;
      try {
        const [courses, sessions, usage, todos] = await Promise.all([
          api.listCourses(),
          api.listSessions(),
          api.listUsage(),
          api.listTodos(),
        ]);
        this.courses = courses;
        this.sessions = sessions;
        this.usage = usage;
        this.todos = todos;
      } finally {
        this.loading = false;
      }
    },
    async reloadCourses() {
      this.courses = await api.listCourses();
    },
    async reloadSessions() {
      this.sessions = await api.listSessions();
    },
    async reloadUsage() {
      this.usage = await api.listUsage();
    },
    async reloadTodos() {
      this.todos = await api.listTodos();
    },
    async addCourse(input: CourseInput): Promise<Course> {
      const c = await api.createCourse(input);
      await this.reloadCourses();
      return c;
    },
    async updateCourse(id: number, input: CourseInput) {
      await api.updateCourse(id, input);
      await this.reloadCourses();
    },
    async removeCourse(id: number) {
      await api.deleteCourse(id);
      this.courses = this.courses.filter((c) => c.id !== id);
      this.sessions = this.sessions.filter((s) => s.courseId !== id);
    },
    async addSession(input: SessionInput): Promise<ClassSession> {
      const s = await api.createSession(input);
      await this.reloadSessions();
      return s;
    },
    /** 某天（YYYY-MM-DD）的课堂记录 */
    sessionsOnDate(dateKey: string): ClassSession[] {
      return this.sessions.filter((s) => toDateKey(new Date(s.startAt)) === dateKey);
    },
  },
});
