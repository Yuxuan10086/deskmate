import type { Period } from "../types";

/** 周一=1 … 周日=7 */
export function weekdayOf(d: Date): number {
  return d.getDay() === 0 ? 7 : d.getDay();
}

/** 返回某天所在周的周一（归零时分秒） */
export function mondayOf(d: Date): Date {
  const w = weekdayOf(d);
  const m = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  m.setDate(m.getDate() - (w - 1));
  return m;
}

export function addDays(d: Date, days: number): Date {
  const r = new Date(d);
  r.setDate(r.getDate() + days);
  return r;
}

/** 解析 YYYY-MM-DD 为本地日期 */
export function parseDate(s: string): Date {
  const [y, m, d] = s.split("-").map(Number);
  return new Date(y, m - 1, d);
}

export function toDateKey(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

/** 依据开学日期（第一周周一）计算某日期的周次 */
export function weekNumberOf(d: Date, semesterStart: string): number {
  const start = mondayOf(parseDate(semesterStart));
  const cur = mondayOf(d);
  const diff = Math.round((cur.getTime() - start.getTime()) / 86400000);
  return Math.floor(diff / 7) + 1;
}

/** 取第 weekNo 周周一到周日的 7 个日期 */
export function datesOfWeek(weekNo: number, semesterStart: string): Date[] {
  const start = mondayOf(parseDate(semesterStart));
  const monday = addDays(start, (weekNo - 1) * 7);
  return Array.from({ length: 7 }, (_, i) => addDays(monday, i));
}

export const WEEKDAY_NAMES = ["", "周一", "周二", "周三", "周四", "周五", "周六", "周日"];
export const WEEKDAY_NAMES_FULL = [
  "",
  "星期一",
  "星期二",
  "星期三",
  "星期四",
  "星期五",
  "星期六",
  "星期日",
];

export function minutesToHM(min: number): string {
  const h = Math.floor(min / 60);
  const m = min % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

export function hmToMinutes(hm: string): number {
  const [h, m] = hm.split(":").map(Number);
  return h * 60 + m;
}

export function formatDateCN(d: Date): string {
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

export function formatDateTime(iso: string): string {
  const d = new Date(iso);
  return `${d.getMonth() + 1}-${String(d.getDate()).padStart(2, "0")} ${String(
    d.getHours()
  ).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

export function formatDuration(sec: number | null): string {
  if (sec == null) return "—";
  const h = Math.floor(sec / 3600);
  const m = Math.round((sec % 3600) / 60);
  return h > 0 ? `${h}小时${m}分钟` : `${m}分钟`;
}

/** 默认节次时段 */
export const DEFAULT_PERIODS: Period[] = [
  { label: "第1–2节", startMinute: 8 * 60, endMinute: 9 * 60 + 35 },
  { label: "第3–4节", startMinute: 10 * 60, endMinute: 11 * 60 + 35 },
  { label: "第5–6节", startMinute: 14 * 60, endMinute: 15 * 60 + 35 },
  { label: "第7–8节", startMinute: 16 * 60, endMinute: 17 * 60 + 35 },
  { label: "第9–10节", startMinute: 19 * 60, endMinute: 20 * 60 + 35 },
];

/** 找到分钟所在节次索引（优先落在时段内，否则取开始时间最近的） */
export function periodIndexOf(minute: number, periods: Period[]): number {
  const inside = periods.findIndex((p) => minute >= p.startMinute && minute <= p.endMinute);
  if (inside >= 0) return inside;
  let best = 0;
  let bestDist = Infinity;
  periods.forEach((p, i) => {
    const dist = Math.min(Math.abs(minute - p.startMinute), Math.abs(minute - p.endMinute));
    if (dist < bestDist) {
      bestDist = dist;
      best = i;
    }
  });
  return best;
}

/** 课程备选配色 */
export const COURSE_COLORS = [
  "#4f6df5",
  "#7c5cff",
  "#0ea5b7",
  "#11c29a",
  "#f59e0b",
  "#ef4444",
  "#ec4899",
  "#64748b",
];

export function pickColor(seed: number): string {
  return COURSE_COLORS[seed % COURSE_COLORS.length];
}
