import { invoke, Channel } from "@tauri-apps/api/core";
import type {
  ActionItem,
  ActionItemEvent,
  AppSettings,
  AudioApp,
  ChatStreamEvent,
  ClassSession,
  Course,
  CourseInput,
  LlmLog,
  Material,
  MaterialCategory,
  ModelTier,
  PresenceChangedEvent,
  PresenceEvent,
  PresenceState,
  PromptTemplate,
  SecretConfig,
  SecretConfigPatch,
  SessionInput,
  SubtitleLineEvent,
  SubtitlePartialEvent,
  SubtitleStatusEvent,
  Todo,
  TranscriptLine,
  TidyPassage,
  UsageRecord,
} from "../types";
import { hmToMinutes, minutesToHM, parseDate, toDateKey, addDays } from "../utils/date";

/** 是否运行在 Tauri 桌面壳内 */
export function isTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * 统一把错误转成可读文案：Tauri 命令的 Err 抛到前端是**字符串**而非 Error 对象，
 * 直接取 `(e as Error).message` 会得到 undefined。
 */
export function errMsg(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

/** 与 Rust pipeline.rs 一致的默认提示词模板（mock 用） */
export const DEFAULT_PROMPTS: Record<string, string> = {
  tidy: "你是课堂文稿整理助手。输入是按序号排列的课堂实时字幕（可能存在被过度切分的短句和口误）。只做两件事：① 把明显属于同一句话的相邻碎片合并成完整语句；② 去掉重复、结巴、口误和无意义的语气词。英文原文严格保持原意、用词和语序：不增删任何信息，不润色，不翻译，不概括，不调整先后的叙述顺序。中文译文在忠实原意的前提下轻度润色：把直译腔调整得通顺自然、符合中文表达习惯，但不得增删实质信息、不得改变句意。一律删除所有表情符号（emoji、颜文字、装饰符号），它们不是讲话内容。只输出一个 JSON 数组，按原文顺序输出整理后的全部内容，每个元素形如 {\"en\":\"整理后的原文\",\"zh\":\"润色后的对应译文（无对应译文则为空字符串）\"}。",
  translate_batch:
    "你是课堂同传助手。把编号的课堂原文逐条翻译为目标语言：保留「编号. 」前缀，每行一条，只输出译文，不要合并或省略行。",
  detect:
    '你是课堂行动项检测器，关注需要学生【当场】处理的事情。需要报告：① 必须当场动手：随堂测验/小测开始需要作答、老师点名提问或要求举手/上台、需要当场提交材料或扫码签到、当场截止的任务；② 可选的课堂互动机会：抢答、课堂小游戏、分组讨论发言等学生可以选择参与的活动。一律不报告：课后才需要完成的作业/复习/预习、老师让学生“听一下/看一下/记一下/想一想/观察/举几个例子”等纯听课要求、普通讲课内容、与课堂事务无关的话。把握尺度要适度：明确符合的必报，拿不准但确有迹象的也可以报告（允许少量误报），只有完全无关才不报。只输出一个 JSON 对象：{"type":"quiz|interaction|deadline|other","content":"事项一句话","action_guide":"建议当场做什么","urgency":"high|medium|low"}；没有则输出 {"type":"none"}。',
  detect_batch:
    "你是课堂行动项检测器。从课堂内容中提取需要学生处理的行动项：课后作业、提交任务、阅读/预习复习、测验考试提醒、需要准备的材料、当场未完成的互动等。每条写清楚任务内容、资源获取方式与提交方式。尺度适度：明确的必报，拿不准但有迹象的也可报告（允许少量误报）。只输出一个 JSON 字符串数组，元素为具体的中文行动项，没有则输出 []。",
  speaker:
    '你是课堂录音说话人标注器。下面是按序号排列的课堂双语字幕，每行末尾标注了同传系统识别到的声道编号（A/B…，? 表示未知）。一节课中讲话最多、持续讲授内容的人是授课老师；其余发言（学生提问与回答、课堂讨论、旁人交谈）为其他人。请逐行判断说话人身份：根据称呼语（“老师/我问一下”等）、内容是讲授还是提问应答来判断；声道切换是重要线索；证据不足、拿不准的一律标为老师。只输出 JSON 数组，每个元素形如 {"seq":12,"who":"teacher"} 或 {"seq":13,"who":"other"}，必须为输入的每一行给出判定，不要输出其他内容。',
  archive:
    '你是课堂归档助手。输入是一节课的双语字幕文稿（按序号排列）与课堂附件信息。请一次性完成两件事。\n一、课后行动项：提取所有需要学生课后或后续处理的事项——课后作业、提交任务、阅读/预习/复习、测验考试提醒、需要准备或携带的材料、登记报名扫码等，也包括当堂没做完的互动任务。\n- 尺度适度：明确的必报，拿不准但确有迹象的也可以列出（允许少量误报）；普通讲课内容不要报。\n- 每条 content 要具体说明是什么任务；action_guide 要写清操作细节：任务内容与范围、需要的资源或材料从哪里获取（课件页码/网络平台/教材章节/邮箱等）、提交方式与渠道、提交格式等，凡上下文能推断的都写上，不要写“按老师要求完成”这种空话。\n- ddl：根据明确的截止表述结合给出的课堂日期推断，输出 "YYYY-MM-DD"（只有相对说法如“下周一”就按课堂日期推算）；无法确定输出 null。\n- urgency：high（三天内/当堂截止）、medium（本周内）、low（更远或无明确期限）。\n二、课堂笔记：把内容整理为结构化 Markdown 笔记，包含课堂大纲、关键知识点、例题与结论，语言精炼。{material_instruction}\n- 笔记篇幅与课堂长度匹配：课堂不足 3 分钟或实质内容过少时 note_markdown 输出空字符串 ""；3–15 分钟约 300–600 字；15–45 分钟约 800–2000 字；45 分钟以上不少于 2000 字。\n{marker_note}只输出一个 JSON 对象，不要输出多余文字：\n{"action_items":[{"type":"homework|submission|reading|exam|material|other","content":"具体任务","action_guide":"操作细节：内容/资源获取/提交方式","urgency":"high|medium|low","ddl":"YYYY-MM-DD 或 null"}],"note_markdown":"Markdown 笔记正文或空字符串"}',
  note: "你是课堂笔记整理助手。把课堂内容整理为结构化的 Markdown 笔记，包含课堂大纲、关键知识点、例题与结论，语言精炼。{material_instruction}",
  chat: '你是「同桌小轩」，正在随堂辅助学生。下面是本节课「{title}」截至目前的增量课堂记录。\n请回答学生的提问（可能附带课堂照片/板书图片，需要你识图理解）。\n判定规则：若这条问答对课后复习有沉淀价值（知识点澄清、题目解答、老师补充、板书讲解等），save_to_note=true；\n简单闲聊、临时操作、与课程无关的问题 save_to_note=false。\n只输出 JSON，不要输出多余内容：{"save_to_note":true或false,"reply":"你的回答，简洁中文，可用 Markdown"}\n\n课堂记录：\n{context}',
};

const PROMPT_LABELS: Record<string, string> = {
  tidy: "文稿整理",
  translate_batch: "归档批量翻译",
  detect: "实时行动项检测",
  detect_batch: "归档行动项检测（旧）",
  speaker: "说话人标注",
  archive: "归档（行动项+笔记）",
  note: "笔记生成",
  chat: "课堂对话",
};

/* ============================== 浏览器 Mock 层 ============================== */

const LS_KEY = "deskmate.db.v1";

interface MockDB {
  seq: number;
  courses: Course[];
  sessions: ClassSession[];
  materials: Material[];
  todos: Todo[];
  usage: UsageRecord[];
  settings: AppSettings;
  seeded: boolean;
}

const DEFAULT_SETTINGS: AppSettings = {
  // 2026-08-24 为周一，2026-10-14（今天）落在第 8 周
  semesterStart: "2026-08-24",
  monthlyBudget: 30,
  speakLanguage: "en",
  dailyLanguage: "zh",
  presenceIntervalSec: 10,
  presenceCameraEnabled: true,
  scheduleStartHour: 8,
  scheduleEndHour: 18,
  micGain: 1.0,
  persona: null,
};

/* --------------------------- 浏览器 Mock 扩展状态 --------------------------- */

/** 密钥配置（浏览器 mock 不加密，仅用于界面联调） */
let mockSecret: SecretConfig = {
  siliconflowKeySet: false,
  siliconflowKeyMasked: "",
  smtpUser: "",
  smtpTo: "",
  smtpAuthSet: false,
  volcKeySet: false,
  volcKeyMasked: "",
};
/** 在位检测：mock 下用 simulate_presence 驱动 */
let mockPresence = { running: false, state: "present" as PresenceState, sessionId: null as number | null };
let mockPresenceEvents: { sessionId: number; ts: string; state: PresenceState }[] = [];
/** 音频采集 mock 状态 */
let mockCapture = {
  running: false,
  sessionId: null as number | null,
  deviceName: "",
  rms: 0,
  silenceWarned: false,
  chunks: 0,
};
/** 提示词覆盖（mock）：task → 模板内容 */
let mockPromptOverrides: Record<string, string> = {};
/** 音频采集目标进程（mock）：null=系统全部声音 */
let mockCaptureTargetPid: number | null = null;
/** 接管课堂目标进程（mock） */
let mockTakenPid: number | null = null;
/** 大模型调用日志（mock，内存态，最多 1000 条） */
let mockLlmLogs: LlmLog[] = [];
let mockLlmLogSeq = 1;

function mockPushLlmLog(entry: Omit<LlmLog, "id" | "ts">): void {
  mockLlmLogs.unshift({ id: mockLlmLogSeq++, ts: new Date().toISOString(), ...entry });
  if (mockLlmLogs.length > 1000) mockLlmLogs.length = 1000;
}

function mockMask(v: string): string {
  if (!v) return "";
  return `${v.slice(0, 3)}********${v.slice(-4)}`;
}

function loadDB(): MockDB {
  const raw = localStorage.getItem(LS_KEY);
  if (raw) {
    const db = JSON.parse(raw) as MockDB;
    // 兼容旧版本数据：补齐新增的课程表时间范围设置、待办与附件分类
    db.settings = { ...DEFAULT_SETTINGS, ...db.settings };
    db.todos ??= [];
    db.materials.forEach((m) => (m.category ??= "slides"));
    return db;
  }
  const db: MockDB = {
    seq: 1,
    courses: [],
    sessions: [],
    materials: [],
    todos: [],
    usage: [],
    settings: { ...DEFAULT_SETTINGS },
    seeded: false,
  };
  seed(db);
  saveDB(db);
  return db;
}

function saveDB(db: MockDB) {
  localStorage.setItem(LS_KEY, JSON.stringify(db));
}

function nextId(db: MockDB): number {
  return db.seq++;
}

function isoAt(d: Date, minute: number): string {
  const r = new Date(d);
  r.setHours(Math.floor(minute / 60), minute % 60, 0, 0);
  return r.toISOString();
}

/**
 * 按课堂时长与模型档位生成确定性用量明细，并以明细汇总回填课堂花费。
 * 金额口径与 Rust 端 pipeline::token_amount 完全一致（元/百万 token）：
 * 归档不再自动生成笔记，故此处不含 note 行（笔记由手动按钮触发）。
 */
const MODEL_PRICE: Record<string, { in: number; out: number; cache: number }> = {
  "GLM-4.5-Air": { in: 1, out: 6, cache: 1 },
  "GLM-4.5V": { in: 1, out: 6, cache: 1 },
  "DeepSeek-V3.2": { in: 4, out: 6, cache: 0.4 },
  "Qwen3.6-27B": { in: 3, out: 18, cache: 3 },
};

function tokenAmount(model: string, input: number, cached: number, out: number): number {
  const p = MODEL_PRICE[model];
  if (!p) return 0;
  const yuan =
    Math.max(input - cached, 0) * p.in + Math.max(cached, 0) * p.cache + Math.max(out, 0) * p.out;
  return Math.round((yuan / 1_000_000) * 10_000) / 10_000;
}

/** 「第 X 讲」：按课表 slots 截至本次上课已排到的第 N 次课（与 Rust lecture_index 一致）。 */
function lectureIndex(
  course: Course,
  weekNo: number,
  weekday: number,
  startMinute: number
): number {
  let n = 0;
  for (const s of course.slots) {
    if (weekNo < course.startWeek) continue;
    n += weekNo - course.startWeek;
    if (s.weekday < weekday || (s.weekday === weekday && s.startMinute <= startMinute)) {
      n += 1;
    }
  }
  return Math.max(1, n);
}

/** 课表变更/新建课堂后重算该课程全部历史讲次与标题。 */
function recountMockCourse(db: MockDB, courseId: number): void {
  const course = db.courses.find((c) => c.id === courseId);
  if (!course) return;
  const start = parseDate(db.settings.semesterStart);
  db.sessions
    .filter((s) => s.courseId === courseId)
    .sort(
      (a, b) =>
        a.weekNo - b.weekNo ||
        a.weekday - b.weekday ||
        a.startMinute - b.startMinute ||
        a.id - b.id
    )
    .forEach((s) => {
      s.lectureNo = lectureIndex(course, s.weekNo, s.weekday, s.startMinute);
      const d = addDays(start, (s.weekNo - 1) * 7 + (s.weekday - 1));
      s.title = `第 ${s.lectureNo} 讲 · ${d.getMonth() + 1}-${String(d.getDate()).padStart(2, "0")}`;
    });
}

function generateUsage(db: MockDB, s: ClassSession): void {
  if (db.usage.some((u) => u.sessionId === s.id)) return;
  const start = new Date(s.startAt).getTime();
  const push = (
    stage: string,
    model: string,
    mins: number,
    input: number,
    cached: number,
    out: number,
    audio = 0
  ): void => {
    db.usage.push({
      id: nextId(db),
      sessionId: s.id,
      ts: new Date(start + mins * 60000).toISOString(),
      stage,
      model,
      modelTier: stage === "asr" ? "free" : s.modelTier,
      inputTokens: input,
      cachedTokens: cached,
      outputTokens: out,
      audioSeconds: audio,
      amount: tokenAmount(model, input, cached, out),
    });
  };
  const dur = (s.durationSec ?? 5400) / 60;
  push("asr", "SenseVoiceSmall", 0, 0, 0, 0, Math.round(s.durationSec ?? 0));
  const blocks = Math.floor(dur / 5);
  for (let i = 0; i < blocks; i++) {
    push("translate", "GLM-4.5-Air", i * 5, 1200, 0, 980);
  }
  const detects = Math.floor(dur / 15);
  for (let i = 0; i < detects; i++) {
    push("detect", "DeepSeek-V3.2", i * 15, 520, 0, 16);
  }

  const total = db.usage
    .filter((u) => u.sessionId === s.id)
    .reduce((sum, u) => sum + u.amount, 0);
  s.costEstimate = Math.round(total * 100) / 100;
}

/** 演示种子数据 */
function seed(db: MockDB) {
  const mkCourse = (
    name: string,
    code: string,
    location: string,
    color: string,
    weekday: number,
    startHM: string,
    endHM: string,
    startWeek = 1,
    endWeek = 16
  ): Course => ({
    id: nextId(db),
    name,
    code,
    location,
    startWeek,
    endWeek,
    color,
    slots: [{ weekday, startMinute: hmToMinutes(startHM), endMinute: hmToMinutes(endHM) }],
    createdAt: new Date().toISOString(),
  });

  const c1 = mkCourse("学术英语写作", "ENGL-102", "文科楼 305", "#4f6df5", 3, "10:00", "11:35");
  const c2 = mkCourse("微积分", "MATH-101", "理科楼 A201", "#0ea5b7", 1, "08:00", "09:35");
  const c3 = mkCourse("宏观经济学", "ECON-201", "经管楼 410", "#f59e0b", 5, "08:00", "09:35");
  const c4 = mkCourse("程序设计基础", "CS-102", "机房 2", "#7c5cff", 2, "10:00", "11:35", 1, 12);
  const c5 = mkCourse("数据结构", "CS-201", "理科楼 B102", "#11c29a", 4, "14:00", "15:35");
  db.courses.push(c1, c2, c3, c4, c5);

  const start = parseDate(db.settings.semesterStart);
  const dateOf = (weekNo: number, weekday: number) => addDays(start, (weekNo - 1) * 7 + (weekday - 1));

  const mkSession = (
    course: Course | null,
    weekNo: number,
    lectureNo: number,
    slotMin: number,
    slotMax: number,
    mode: "online" | "offline",
    tier: ModelTier,
    durationMin: number
  ): ClassSession => {
    const weekday = course ? course.slots[0].weekday : 1;
    const d = dateOf(weekNo, weekday);
    return {
      id: nextId(db),
      courseId: course?.id ?? null,
      title: `第 ${lectureNo} 讲 · ${d.getMonth() + 1}-${String(d.getDate()).padStart(2, "0")}`,
      lectureNo,
      startAt: isoAt(d, slotMin),
      endAt: isoAt(d, slotMax),
      weekday,
      weekNo,
      startMinute: slotMin,
      endMinute: slotMax,
      location: course?.location ?? null,
      mode,
      modelTier: tier,
      status: "finished",
      durationSec: durationMin * 60,
      costEstimate: 0,
      note: null,
    };
  };

  // 历史课堂（costEstimate 由用量明细汇总回填，见下方）
  const s1 = mkSession(c1, 5, 5, 600, 695, "offline", "balanced", 92);
  const s2 = mkSession(c1, 6, 6, 600, 695, "offline", "balanced", 95);
  const s3 = mkSession(c1, 7, 7, 600, 695, "online", "balanced", 93);
  const s4 = mkSession(c1, 8, 8, 600, 695, "offline", "balanced", 90);
  const s5 = mkSession(c2, 7, 7, 480, 575, "offline", "eco", 92);
  const s6 = mkSession(c2, 8, 8, 480, 575, "offline", "eco", 95);
  const s7 = mkSession(c4, 7, 7, 600, 695, "online", "free", 94);
  const s8 = mkSession(c4, 8, 8, 600, 695, "online", "free", 96);
  const s9 = mkSession(c5, 7, 7, 840, 935, "offline", "pro", 93);
  db.sessions.push(s1, s2, s3, s4, s5, s6, s7, s8, s9);

  // 用量明细（确定性生成）；课堂花费以明细汇总为准
  for (const s of db.sessions) generateUsage(db, s);

  db.materials.push({
    id: nextId(db),
    courseId: c1.id,
    sessionId: s4.id,
    fileName: "Lecture8_Academic Writing.pptx",
    fileType: "pptx",
    sizeBytes: 4_820_000,
    filePath: "",
    confidence: "high",
    category: "slides",
    createdAt: new Date().toISOString(),
  });

  db.seeded = true;
  void toDateKey; // 保留工具引用
}

/* ------------------------------ Mock 命令处理 ------------------------------ */

function mockHandler(cmd: string, args: Record<string, unknown> | undefined): unknown {
  const db = loadDB();
  const arg = (k: string) => (args?.[k] as never) ?? undefined;

  switch (cmd) {
    case "list_courses":
      return db.courses.sort((a, b) => a.createdAt.localeCompare(b.createdAt));

    case "create_course": {
      const input = arg("input") as CourseInput;
      const c: Course = {
        id: nextId(db),
        name: input.name,
        code: input.code ?? null,
        location: input.location ?? null,
        startWeek: input.startWeek,
        endWeek: input.endWeek,
        color: input.color ?? "#4f6df5",
        slots: input.slots,
        createdAt: new Date().toISOString(),
      };
      db.courses.push(c);
      saveDB(db);
      return c;
    }

    case "update_course": {
      const id = arg("id") as number;
      const input = arg("input") as CourseInput;
      const c = db.courses.find((x) => x.id === id);
      if (!c) throw new Error("课程不存在");
      Object.assign(c, {
        name: input.name,
        code: input.code ?? null,
        location: input.location ?? null,
        startWeek: input.startWeek,
        endWeek: input.endWeek,
        color: input.color ?? c.color,
        slots: input.slots,
      });
      saveDB(db);
      recountMockCourse(db, id);
      saveDB(db);
      return c;
    }

    case "delete_course": {
      const id = arg("id") as number;
      const removedSessionIds = new Set(
        db.sessions.filter((s) => s.courseId === id).map((s) => s.id)
      );
      db.courses = db.courses.filter((c) => c.id !== id);
      db.sessions = db.sessions.filter((s) => s.courseId !== id);
      // 级联清理：该课程的用量明细与课件（与 Rust 端外键 ON DELETE CASCADE 行为一致）
      db.usage = db.usage.filter((u) => u.sessionId == null || !removedSessionIds.has(u.sessionId));
      db.materials = db.materials.filter((m) => m.courseId !== id);
      saveDB(db);
      return null;
    }

    case "list_sessions": {
      const courseId = arg("courseId") as number | undefined;
      let list = [...db.sessions];
      if (courseId != null) list = list.filter((s) => s.courseId === courseId);
      return list.sort((a, b) => b.startAt.localeCompare(a.startAt));
    }

    case "get_session":
      return db.sessions.find((s) => s.id === (arg("id") as number)) ?? null;

    case "create_session": {
      const input = arg("input") as SessionInput;
      const course = db.courses.find((c) => c.id === input.courseId) ?? null;
      const start = parseDate(db.settings.semesterStart);
      const d = addDays(start, (input.weekNo - 1) * 7 + (input.weekday - 1));
      const lectureNo = course
        ? lectureIndex(course, input.weekNo, input.weekday, input.startMinute)
        : db.sessions.filter((s) => s.courseId === null).length + 1;
      const s: ClassSession = {
        id: nextId(db),
        courseId: input.courseId,
        title: `第 ${lectureNo} 讲 · ${d.getMonth() + 1}-${String(d.getDate()).padStart(2, "0")}`,
        lectureNo,
        startAt: input.startAt ?? isoAt(d, input.startMinute),
        endAt: null,
        weekday: input.weekday,
        weekNo: input.weekNo,
        startMinute: input.startMinute,
        endMinute: input.endMinute,
        location: input.location ?? course?.location ?? null,
        mode: input.mode,
        modelTier: input.modelTier,
        status: "recording",
        durationSec: null,
        costEstimate: 0,
        note: null,
      };
      db.sessions.push(s);
      saveDB(db);
      return s;
    }

    case "finish_session": {
      const id = arg("id") as number;
      const payload = arg("payload") as { durationSec: number; costEstimate: number; note?: string };
      const s = db.sessions.find((x) => x.id === id);
      if (!s) throw new Error("课堂不存在");
      s.status = "finished";
      // 续录再归档：时长在历史值上累加
      s.durationSec = (s.durationSec ?? 0) + payload.durationSec;
      s.note = payload.note ?? s.note;
      s.endAt = new Date().toISOString();
      // mock 管线：归档时补齐 asr/translate/detect 用量（笔记不自动生成）
      generateUsage(db, s);
      saveDB(db);
      return s;
    }

    case "resume_session": {
      const id = arg("id") as number;
      const s = db.sessions.find((x) => x.id === id);
      if (!s) throw new Error("课堂不存在");
      if (s.status !== "finished") throw new Error("仅已归档课堂可以继续录制");
      s.status = "recording";
      s.endAt = null;
      saveDB(db);
      return s;
    }

    case "start_capture": {
      const sessionId = arg("sessionId") as number;
      const mode = (arg("mode") as string) ?? "offline";
      const targetPid = (arg("targetPid") as number | undefined) ?? null;
      const appName = targetPid != null ? `应用 ${targetPid}（mock）` : "模拟扬声器（mock）";
      mockCapture = {
        running: true,
        sessionId,
        deviceName:
          mode === "online" ? `系统声音 · ${appName}` : `模拟麦克风（mock · 增益生效中）`,
        rms: 1200,
        silenceWarned: false,
        chunks: 0,
      };
      mockCaptureTargetPid = targetPid;
      return true;
    }

    case "stop_capture":
      mockCapture = { ...mockCapture, running: false };
      mockCaptureTargetPid = null;
      return null;

    case "capture_status":
      return {
        ...mockCapture,
        targetPid: mockCaptureTargetPid,
        mutedPid: mockTakenPid,
        captureNote: null,
      };

    case "list_audio_apps": {
      // mock：模拟两个正在播放声音的应用
      return [
        { pid: 4321, name: "msedge", active: true },
        { pid: 8765, name: "vlc", active: false },
      ] satisfies AudioApp[];
    }

    case "take_over_class":
      mockTakenPid = (arg("pid") as number) ?? null;
      return 1;

    case "release_class":
      mockTakenPid = null;
      return null;

    case "set_mic_gain":
      return Math.min(4, Math.max(0.5, (arg("gain") as number) ?? 1));

    case "regenerate_note":
    case "generate_note_now": {
      const id = arg("id") as number;
      const s = db.sessions.find((x) => x.id === id);
      if (!s) throw new Error("课堂不存在");
      s.note =
        `# ${s.title} · 课堂笔记（mock）\n\n` +
        `## 课堂大纲\n- 知识点一：示例概念\n- 知识点二：示例应用\n\n` +
        `## 关键结论\n1. 示例结论 A\n2. 示例结论 B\n\n## 例题\n- 例题与解答要点（mock 数据）`;
      const model = s.modelTier === "pro" ? "Qwen3.6-27B" : "DeepSeek-V3.2";
      db.usage.push({
        id: nextId(db),
        sessionId: s.id,
        ts: new Date().toISOString(),
        stage: "note",
        model,
        modelTier: s.modelTier,
        inputTokens: 2800,
        cachedTokens: 0,
        outputTokens: 600,
        audioSeconds: 0,
        amount: tokenAmount(model, 2800, 0, 600),
      });
      s.costEstimate = Math.round(
        db.usage.filter((u) => u.sessionId === s.id).reduce((sum, u) => sum + u.amount, 0) * 100
      ) / 100;
      mockPushLlmLog({
        sessionId: s.id,
        stage: "note",
        model,
        systemPrompt: (mockPromptOverrides["note"] ?? DEFAULT_PROMPTS["note"]).replace(
          "{material_instruction}",
          ""
        ),
        userPrompt: `（mock）${s.title} 的双语转写全文`,
        response: s.note,
        ok: true,
        error: null,
        inputTokens: 2800,
        cachedTokens: 0,
        outputTokens: 600,
      });
      saveDB(db);
      return s;
    }

    case "deskmate_chat_stream": {
      const text = (arg("text") as string) ?? "";
      const hasImage = !!arg("imageDataUrl");
      const sessionId = (arg("sessionId") as number | null) ?? null;
      const s = sessionId != null ? db.sessions.find((x) => x.id === sessionId) : undefined;
      if (sessionId != null && !s) throw new Error("课堂不存在");
      const model = hasImage ? "GLM-4.5V" : "DeepSeek-V3.2";
      if (s) {
        db.usage.push({
          id: nextId(db),
          sessionId,
          ts: new Date().toISOString(),
          stage: "chat",
          model,
          modelTier: s.modelTier,
          inputTokens: 500,
          cachedTokens: 0,
          outputTokens: 120,
          audioSeconds: 0,
          amount: tokenAmount(model, 500, 0, 120),
        });
        s.costEstimate = Math.round(
          db.usage
            .filter((u) => u.sessionId === sessionId)
            .reduce((sum, u) => sum + u.amount, 0) * 100
        ) / 100;
      }
      const mockReply = `（浏览器 mock 回复）关于「${text.slice(0, 40) || "图片/文件"}」的模拟回答，桌面端将调用真实模型流式回答并按页面范围注入上下文。`;
      mockPushLlmLog({
        sessionId,
        stage: "chat",
        model,
        systemPrompt: `${mockPromptOverrides["chat"] ?? DEFAULT_PROMPTS["chat"]}\n（mock，scope=${arg("scope")}）`,
        userPrompt: hasImage ? `${text} [附图片/文件]` : text,
        response: mockReply,
        ok: true,
        error: null,
        inputTokens: 500,
        cachedTokens: 0,
        outputTokens: 120,
      });
      saveDB(db);
      return mockReply;
    }

    /* ------------------------------- 待办 ------------------------------- */

    case "list_todos":
      return [...db.todos].sort((a, b) => {
        if (a.done !== b.done) return a.done ? 1 : -1;
        if (a.ddl == null && b.ddl != null) return -1;
        if (a.ddl != null && b.ddl == null) return 1;
        if (a.ddl != null && b.ddl != null && a.ddl !== b.ddl) return a.ddl.localeCompare(b.ddl);
        return a.createdAt.localeCompare(b.createdAt);
      });

    case "create_todo": {
      const input = arg("input") as {
        sessionId?: number | null;
        title: string;
        detail?: string | null;
        ddl?: string | null;
      };
      const todo: Todo = {
        id: nextId(db),
        sessionId: input.sessionId ?? null,
        actionItemId: null,
        source: "manual",
        title: input.title,
        detail: input.detail ?? null,
        ddl: input.ddl ?? null,
        done: false,
        createdAt: new Date().toISOString(),
        completedAt: null,
      };
      db.todos.push(todo);
      saveDB(db);
      return todo;
    }

    case "update_todo": {
      const input = arg("input") as { id: number; title?: string; detail?: string | null; ddl?: string | null };
      const t = db.todos.find((x) => x.id === input.id);
      if (!t) throw new Error("待办不存在");
      if (input.title !== undefined) t.title = input.title;
      if (input.detail !== undefined) t.detail = input.detail;
      if (input.ddl !== undefined) t.ddl = input.ddl;
      saveDB(db);
      return t;
    }

    case "set_todo_done": {
      const id = arg("id") as number;
      const done = arg("done") as boolean;
      const t = db.todos.find((x) => x.id === id);
      if (!t) throw new Error("待办不存在");
      t.done = done;
      t.completedAt = done ? new Date().toISOString() : null;
      saveDB(db);
      return null;
    }

    case "delete_todo": {
      const id = arg("id") as number;
      db.todos = db.todos.filter((x) => x.id !== id);
      saveDB(db);
      return null;
    }

    case "set_action_item_ddl":
      return null;

    case "open_material":
      return null;

    case "delete_material": {
      const id = arg("id") as number;
      db.materials = db.materials.filter((x) => x.id !== id);
      saveDB(db);
      return null;
    }

    case "respond_note_save":
      return true;

    case "list_tidy_passages":
      return [];

    case "list_materials": {
      const courseId = arg("courseId") as number;
      return db.materials.filter((m) => m.courseId === courseId);
    }

    case "upload_material": {
      const input = arg("input") as {
        fileName: string;
        base64?: string;
        dataBase64?: string;
        courseId: number;
        sessionId?: number;
        category?: MaterialCategory;
      };
      // mock：解析 base64 取长度、按扩展名判定类型
      const rawB64 = input.dataBase64 ?? input.base64 ?? "";
      const comma = rawB64.indexOf(",");
      const body = comma >= 0 ? rawB64.slice(comma + 1) : rawB64;
      const sizeBytes = Math.floor((body.length * 3) / 4);
      const ext = input.fileName.split(".").pop()?.toLowerCase() ?? "bin";
      const fileType = ["pptx", "ppt", "pdf"].includes(ext) ? ext : ext;
      const category: MaterialCategory =
        input.category ?? (["pptx", "ppt", "pdf"].includes(ext) ? "slides" : "other");
      const mat: Material = {
        id: nextId(db),
        courseId: input.courseId,
        sessionId: input.sessionId ?? null,
        fileName: input.fileName,
        fileType,
        sizeBytes,
        filePath: "",
        confidence: "manual",
        category,
        createdAt: new Date().toISOString(),
      };
      db.materials.push(mat);
      saveDB(db);
      return { material: mat, pageCount: 0, keywords: [], confidence: "manual" };
    }

    case "list_usage":
      return db.usage.sort((a, b) => b.ts.localeCompare(a.ts));

    case "get_settings":
      return db.settings;

    case "save_settings": {
      db.settings = { ...db.settings, ...(arg("input") as Partial<AppSettings>) };
      saveDB(db);
      return db.settings;
    }

    case "get_secret_config":
      return { ...mockSecret };

    case "save_secret_config": {
      const patch = (arg("patch") ?? {}) as SecretConfigPatch;
      if (patch.clearSiliconflow) {
        mockSecret = { ...mockSecret, siliconflowKeySet: false, siliconflowKeyMasked: "" };
      } else if (patch.siliconflowKey) {
        mockSecret = {
          ...mockSecret,
          siliconflowKeySet: true,
          siliconflowKeyMasked: mockMask(patch.siliconflowKey),
        };
      }
      if (patch.clearVolc) {
        mockSecret = { ...mockSecret, volcKeySet: false, volcKeyMasked: "" };
      } else if (patch.volcKey) {
        mockSecret = {
          ...mockSecret,
          volcKeySet: true,
          volcKeyMasked: mockMask(patch.volcKey),
        };
      }
      if (patch.clearSmtp) {
        mockSecret = { ...mockSecret, smtpUser: "", smtpTo: "", smtpAuthSet: false };
      } else {
        if (patch.smtpUser !== undefined) mockSecret.smtpUser = patch.smtpUser;
        if (patch.smtpTo !== undefined) mockSecret.smtpTo = patch.smtpTo;
        if (patch.smtpAuthCode) mockSecret.smtpAuthSet = true;
      }
      return { ...mockSecret };
    }

    case "test_siliconflow": {
      if (!mockSecret.siliconflowKeySet) throw new Error("未配置硅基流动 API Key");
      return "连接成功（mock）";
    }

    case "test_volc": {
      if (!mockSecret.volcKeySet) throw new Error("未配置火山引擎 API Key");
      return "连接成功（mock）";
    }

    case "test_mail": {
      if (!mockSecret.smtpUser || !mockSecret.smtpAuthSet) {
        throw new Error("QQ 邮箱未配置：请填写邮箱地址与 SMTP 授权码");
      }
      return null;
    }

    case "send_reminder_email":
      // mock 下不做真实发送
      console.info("[mock] 发送提醒邮件", arg("subject"), arg("body"));
      return null;

    case "start_presence": {
      const sessionId = arg("sessionId") as number;
      mockPresence = { running: true, state: "present", sessionId };
      return true;
    }

    case "stop_presence":
      mockPresence = { running: false, state: mockPresence.state, sessionId: mockPresence.sessionId };
      return null;

    case "presence_status":
      return { ...mockPresence, detail: null };

    case "probe_camera":
      return { ok: true, faces: false, detail: "mock 模式：假设摄像头不可用" };

    case "simulate_presence": {
      const hint = arg("hint") as number;
      const next: PresenceState = hint === 1 ? "away" : hint === 2 ? "unavailable" : "present";
      if (next !== mockPresence.state) {
        mockPresence.state = next;
        if (mockPresence.sessionId != null) {
          mockPresenceEvents.push({
            sessionId: mockPresence.sessionId,
            ts: new Date().toISOString(),
            state: next,
          });
        }
      }
      return null;
    }

    case "list_presence_events": {
      const sessionId = arg("sessionId") as number;
      return mockPresenceEvents
        .filter((e) => e.sessionId === sessionId)
        .map((e) => ({ ts: e.ts, state: e.state }))
        .sort((a, b) => b.ts.localeCompare(a.ts));
    }

    case "list_transcript":
      return [];

    case "list_action_items":
      return [];

    case "mark_action_item": {
      // mock 无行动项数据：返回空（真实端首次加入会创建待办并返回 Todo）
      return null;
    }

    case "start_live":
      // 浏览器 mock：无实时字幕通道
      return false;

    case "stop_live":
      return null;

    /* --------------------------- 提示词模板 --------------------------- */

    case "get_prompt_templates": {
      return Object.keys(DEFAULT_PROMPTS).map((task) => {
        const override = mockPromptOverrides[task];
        return {
          task,
          label: PROMPT_LABELS[task] ?? task,
          template: override ?? DEFAULT_PROMPTS[task],
          isDefault: override == null,
        } satisfies PromptTemplate;
      });
    }

    case "save_prompt_template": {
      const task = arg("task") as string;
      const template = (arg("template") as string) ?? "";
      if (!(task in DEFAULT_PROMPTS)) throw new Error(`未知任务：${task}`);
      if (!template.trim()) throw new Error("模板内容不能为空");
      if (template === DEFAULT_PROMPTS[task]) {
        delete mockPromptOverrides[task];
      } else {
        mockPromptOverrides[task] = template;
      }
      return null;
    }

    case "reset_prompt_templates":
      mockPromptOverrides = {};
      return null;

    /* --------------------------- 大模型调用日志 --------------------------- */

    case "list_llm_logs": {
      const limit = Math.min(Math.max((arg("limit") as number) ?? 200, 1), 1000);
      return mockLlmLogs.slice(0, limit);
    }

    case "clear_llm_logs":
      mockLlmLogs = [];
      return null;

    case "open_captures_dir":
      // 浏览器环境无法打开本地目录，返回模拟路径供提示
      return "（浏览器 mock）%APPDATA%\\com.deskmate.app\\captures";

    default:
      throw new Error(`未知命令（mock）: ${cmd}`);
  }
}

/* -------------------------------- 统一入口 -------------------------------- */

export async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (isTauri()) {
    return await invoke<T>(cmd, args);
  }
  // 模拟异步
  return new Promise((resolve, reject) => {
    setTimeout(() => {
      try {
        resolve(mockHandler(cmd, args) as T);
      } catch (e) {
        reject(e);
      }
    }, 30);
  });
}

/* -------------------------------- 语义化 API ------------------------------- */

export const api = {
  listCourses: () => call<Course[]>("list_courses"),
  createCourse: (input: CourseInput) => call<Course>("create_course", { input }),
  updateCourse: (id: number, input: CourseInput) => call<Course>("update_course", { id, input }),
  deleteCourse: (id: number) => call<null>("delete_course", { id }),

  listSessions: (courseId?: number) =>
    call<ClassSession[]>("list_sessions", courseId != null ? { courseId } : {}),
  getSession: (id: number) => call<ClassSession | null>("get_session", { id }),
  createSession: (input: SessionInput) => call<ClassSession>("create_session", { input }),
  finishSession: (id: number, payload: { durationSec: number; costEstimate: number; note?: string }) =>
    call<ClassSession>("finish_session", { id, payload }),

  // 课堂回收站
  listDeletedSessions: () => call<ClassSession[]>("list_deleted_sessions", {}),
  softDeleteSession: (id: number) => call<null>("soft_delete_session", { id }),
  restoreSession: (id: number) => call<null>("restore_session", { id }),
  purgeSession: (id: number) => call<null>("purge_session", { id }),

  listMaterials: (courseId: number) => call<Material[]>("list_materials", { courseId }),
  uploadMaterial: (input: {
    fileName: string;
    base64: string;
    courseId: number;
    sessionId?: number;
    category?: MaterialCategory;
  }) =>
    call<{ material: Material; pageCount: number; keywords: string[]; confidence: string }>(
      "upload_material",
      { input }
    ),
  /** 用系统默认程序打开附件 */
  openMaterial: (id: number) => call<string>("open_material", { id }),
  /** 删除附件（文件 + 记录） */
  deleteMaterial: (id: number) => call<null>("delete_material", { id }),

  /* -------------------------------- 待办 -------------------------------- */
  listTodos: () => call<Todo[]>("list_todos"),
  createTodo: (input: {
    sessionId?: number | null;
    title: string;
    detail?: string | null;
    ddl?: string | null;
  }) => call<Todo>("create_todo", { input }),
  updateTodo: (input: {
    id: number;
    title?: string;
    detail?: string | null;
    ddl?: string | null;
  }) => call<Todo>("update_todo", { input }),
  setTodoDone: (id: number, done: boolean) => call<null>("set_todo_done", { id, done }),
  deleteTodo: (id: number) => call<null>("delete_todo", { id }),
  /** 手动设置/修改行动项 DDL（YYYY-MM-DD 或 RFC3339；空串清除） */
  setActionItemDdl: (id: number, ddl: string) =>
    call<null>("set_action_item_ddl", { id, ddl }),
  /** 答复小轩「记入笔记」确认卡 */
  respondNoteSave: (requestId: string, approved: boolean) =>
    call<boolean>("respond_note_save", { input: { requestId, approved } }),

  listUsage: () => call<UsageRecord[]>("list_usage"),
  getSettings: () => call<AppSettings>("get_settings"),
  saveSettings: (input: Partial<AppSettings>) => call<AppSettings>("save_settings", { input }),

  startCapture: (sessionId: number, mode: "online" | "offline", targetPid?: number | null) =>
    call<boolean>("start_capture", { sessionId, mode, targetPid: targetPid ?? null }),
  stopCapture: () => call<void>("stop_capture"),
  captureStatus: () =>
    call<{
      running: boolean;
      sessionId: number | null;
      deviceName: string;
      rms: number;
      silenceWarned: boolean;
      chunks: number;
      targetPid: number | null;
      mutedPid: number | null;
      /** 采集异常提示（如按应用采集拿不到声音已自动回退系统声音） */
      captureNote: string | null;
      /** 当前字幕引擎：ast=同传 cheap=经济转写 off=关闭 */
      engine: "ast" | "cheap" | "off";
    }>("capture_status"),

  /* --------------------------- 按应用采集 / 接管课堂 --------------------------- */
  /** 列出默认输出设备上正在播放声音的应用（网课按应用采集候选） */
  listAudioApps: () => call<AudioApp[]>("list_audio_apps"),
  /** 接管课堂：静音目标应用在本机的声音输出（进程回环录音不受影响），返回静音的会话数 */
  takeOverClass: (pid: number) => call<number>("take_over_class", { pid }),
  /** 取消接管：恢复目标应用的声音输出 */
  releaseClass: () => call<null>("release_class"),
  /** 设置麦克风软件增益（0.5–4.0，实时生效并持久化），返回实际生效值 */
  setMicGain: (gain: number) => call<number>("set_mic_gain", { gain }),

  /* ------------------------------ 密钥配置 ------------------------------ */
  getSecretConfig: () => call<SecretConfig>("get_secret_config"),
  saveSecretConfig: (patch: SecretConfigPatch) =>
    call<SecretConfig>("save_secret_config", { patch }),
  testSiliconflow: () => call<string>("test_siliconflow"),
  testVolc: () => call<string>("test_volc"),
  testMail: () => call<null>("test_mail"),
  sendReminderEmail: (subject: string, body: string) =>
    call<null>("send_reminder_email", { subject, body }),

  /* ------------------------------ 在位检测 ------------------------------ */
  startPresence: (sessionId: number) => call<boolean>("start_presence", { sessionId }),
  stopPresence: () => call<null>("stop_presence"),
  presenceStatus: () =>
    call<{
      running: boolean;
      state: PresenceState | null;
      sessionId: number | null;
      /** 摄像头不可用等异常原因（正常为 null） */
      detail: string | null;
    }>("presence_status"),
  /** 单次探测摄像头人脸检测是否可用（设置页「测试摄像头」） */
  probeCamera: () =>
    call<{ ok: boolean; faces: boolean | null; detail: string | null }>("probe_camera"),
  /** 注入采样帧用于无摄像头环境验证：0=在位 1=缺席 2=不可用 3=恢复摄像头自动检测 */
  simulatePresence: (hint: 0 | 1 | 2 | 3) => call<null>("simulate_presence", { hint }),
  listPresenceEvents: (sessionId: number) =>
    call<PresenceEvent[]>("list_presence_events", { sessionId }),

  /* --------------------------- 实时字幕/行动项 --------------------------- */
  listTranscript: (sessionId: number) =>
    call<TranscriptLine[]>("list_transcript", { sessionId }),
  listActionItems: (sessionId: number) =>
    call<ActionItem[]>("list_action_items", { sessionId }),
  /** 行动项加入待办（幂等）：首次加入返回新建的 Todo */
  markActionItem: (id: number) => call<Todo | null>("mark_action_item", { id }),
  /** 取消加入待办：删除对应待办并清除行动项标记 */
  unmarkActionItem: (id: number) => call<null>("unmark_action_item", { id }),
  /** 启动实时字幕线程；返回 false 表示未配置 API Key */
  startLive: (sessionId: number) => call<boolean>("start_live", { sessionId }),
  stopLive: () => call<null>("stop_live"),
  /** 录制中热切换字幕引擎：ast=豆包同传 / cheap=每 10 秒经济转写 / off=关闭字幕，返回实际引擎 */
  setLiveEngine: (sessionId: number, engine: "ast" | "cheap" | "off") =>
    call<"ast" | "cheap" | "off">("set_live_engine", { sessionId, engine }),
  /** 手动重新生成笔记（归档时已按课堂长度自动生成；短于 3 分钟的课堂不生成） */
  regenerateNote: (id: number) => call<ClassSession>("regenerate_note", { id }),
  /** 归档后继续录制 */
  resumeSession: (id: number) => call<ClassSession>("resume_session", { id }),
  // deskmateChatStream 见文件底部独立导出（需要 Channel 回调）
  /** 课堂整理稿（文稿整理结果） */
  listTidyPassages: (sessionId: number) =>
    call<TidyPassage[]>("list_tidy_passages", { sessionId }),

  /* --------------------------- 提示词模板 --------------------------- */
  getPromptTemplates: () => call<PromptTemplate[]>("get_prompt_templates"),
  savePromptTemplate: (task: string, template: string) =>
    call<null>("save_prompt_template", { task, template }),
  resetPromptTemplates: () => call<null>("reset_prompt_templates"),

  /* --------------------------- 大模型调用日志 --------------------------- */
  listLlmLogs: (limit?: number) => call<LlmLog[]>("list_llm_logs", limit != null ? { limit } : {}),
  clearLlmLogs: () => call<null>("clear_llm_logs"),

  /** 打开课堂录音原音频目录，返回目录完整路径 */
  openCapturesDir: (sessionId?: number) =>
    call<string>("open_captures_dir", sessionId != null ? { sessionId } : {}),
};

/** 监听后端在位的状态变化事件（仅 Tauri 环境）；返回取消监听函数。 */
export async function onPresenceChanged(
  cb: (e: PresenceChangedEvent) => void
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return await listen<PresenceChangedEvent>("presence://changed", (ev) => cb(ev.payload));
}

/** 监听实时字幕新行（仅 Tauri 环境）。 */
export async function onSubtitleLine(cb: (e: SubtitleLineEvent) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return await listen<SubtitleLineEvent>("subtitle://line", (ev) => cb(ev.payload));
}

/** 监听 AST 流式部分字幕（未定稿行，边说边出字）。 */
export async function onSubtitlePartial(cb: (e: SubtitlePartialEvent) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return await listen<SubtitlePartialEvent>("subtitle://partial", (ev) => cb(ev.payload));
}

/** 监听字幕通道状态提示（仅 Tauri 环境）。 */
export async function onSubtitleStatus(cb: (e: SubtitleStatusEvent) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return await listen<SubtitleStatusEvent>("subtitle://status", (ev) => cb(ev.payload));
}

/** 监听新行动项（仅 Tauri 环境）。 */
export async function onActionItemNew(cb: (e: ActionItemEvent) => void): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return await listen<ActionItemEvent>("actionitem://new", (ev) => cb(ev.payload));
}

/* ------------------------------ 小轩流式聊天 ------------------------------ */

export interface DeskmateChatArgs {
  scope: "dashboard" | "course" | "session";
  courseId?: number | null;
  sessionId?: number | null;
  history: { role: "user" | "assistant"; content: string }[];
  text: string;
  /** 图片 dataURL 或文本附件内容（由调用方拼好） */
  imageDataUrl?: string | null;
}

/**
 * 同桌小轩流式聊天：onEvent 实时收到 delta/noteRequest 等事件，
 * 最终返回完整回复与是否有内容经批准写入笔记。
 */
export async function deskmateChatStream(
  args: DeskmateChatArgs,
  onEvent: (e: ChatStreamEvent) => void
): Promise<{ reply: string; savedToNote: boolean }> {
  if (isTauri()) {
    const channel = new Channel<ChatStreamEvent>();
    channel.onmessage = (msg) => onEvent(msg);
    return await invoke<{ reply: string; savedToNote: boolean }>("deskmate_chat_stream", {
      scope: args.scope,
      courseId: args.courseId ?? null,
      sessionId: args.sessionId ?? null,
      history: args.history,
      text: args.text,
      imageDataUrl: args.imageDataUrl ?? null,
      onEvent: channel,
    });
  }
  // 浏览器 mock：先记日志/用量，再模拟打字机流式输出
  const reply = await call<string>("deskmate_chat_stream", {
    scope: args.scope,
    courseId: args.courseId ?? null,
    sessionId: args.sessionId ?? null,
    history: args.history,
    text: args.text,
    imageDataUrl: args.imageDataUrl ?? null,
  });
  onEvent({ type: "status", stage: "thinking" });
  const chars = Array.from(reply);
  for (let i = 0; i < chars.length; i += 2) {
    onEvent({ type: "delta", text: chars.slice(i, i + 2).join("") });
    await new Promise((r) => setTimeout(r, 12));
  }
  return { reply, savedToNote: false };
}

export { minutesToHM };
