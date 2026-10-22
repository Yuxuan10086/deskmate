import type { Course, CourseTimeSlot } from "../types";
import { DEFAULT_PERIODS, periodIndexOf, weekNumberOf } from "./date";

export interface Prefill {
  courseId: number | null;
  weekNo: number;
  weekday: number;
  startMinute: number;
  endMinute: number;
  location: string | null;
  /** 预填命中的置信说明 */
  matched: boolean;
}

/** 依据系统时间与课程表预填"这是哪节课" */
export function prefillSession(
  now: Date,
  courses: Course[],
  semesterStart: string
): Prefill {
  const weekNo = weekNumberOf(now, semesterStart);
  const weekday = now.getDay() === 0 ? 7 : now.getDay();
  const minute = now.getHours() * 60 + now.getMinutes();

  let best: { course: Course; slot: CourseTimeSlot; dist: number } | null = null;

  for (const c of courses) {
    if (weekNo < c.startWeek || weekNo > c.endWeek) continue;
    for (const slot of c.slots) {
      if (slot.weekday !== weekday) continue;
      const dist =
        minute >= slot.startMinute && minute <= slot.endMinute
          ? 0
          : Math.min(Math.abs(minute - slot.startMinute), Math.abs(minute - slot.endMinute));
      if (!best || dist < best.dist) best = { course: c, slot, dist };
    }
  }

  // 课间或课前 60 分钟内仍算命中；超过则只给默认节次、不预选课程
  if (best && best.dist <= 60) {
    return {
      courseId: best.course.id,
      weekNo,
      weekday,
      startMinute: best.slot.startMinute,
      endMinute: best.slot.endMinute,
      location: best.course.location,
      matched: best.dist === 0,
    };
  }

  const p = DEFAULT_PERIODS[periodIndexOf(minute, DEFAULT_PERIODS)];
  return {
    courseId: null,
    weekNo,
    weekday,
    startMinute: p.startMinute,
    endMinute: p.endMinute,
    location: null,
    matched: false,
  };
}
