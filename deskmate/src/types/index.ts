/** 周内上课时段：weekday 1=周一 … 7=周日；分钟数 0–1439 */
export interface CourseTimeSlot {
  weekday: number;
  startMinute: number;
  endMinute: number;
}

export interface Course {
  id: number;
  name: string;
  code: string | null;
  location: string | null;
  startWeek: number;
  endWeek: number;
  /** 课程主题色（封面/课程表用） */
  color: string;
  slots: CourseTimeSlot[];
  createdAt: string;
}

export interface CourseInput {
  name: string;
  code?: string | null;
  location?: string | null;
  startWeek: number;
  endWeek: number;
  color?: string;
  slots: CourseTimeSlot[];
}

export type SessionMode = "online" | "offline";
export type SessionStatus = "recording" | "finished";
export type ModelTier = "free" | "eco" | "balanced" | "pro" | "custom";

export interface ClassSession {
  id: number;
  courseId: number | null;
  title: string;
  lectureNo: number;
  startAt: string;
  endAt: string | null;
  weekday: number;
  weekNo: number;
  startMinute: number;
  endMinute: number;
  location: string | null;
  mode: SessionMode;
  modelTier: ModelTier;
  status: SessionStatus;
  durationSec: number | null;
  costEstimate: number;
  note: string | null;
  /** 回收站：null=正常；字符串=软删除时间 */
  deletedAt?: string | null;
}

export interface SessionInput {
  courseId: number | null;
  mode: SessionMode;
  modelTier: ModelTier;
  weekday: number;
  weekNo: number;
  startMinute: number;
  endMinute: number;
  location?: string | null;
  startAt?: string;
}

/** 附件分类：课件 PPT / 作业题目 / 其他附件 */
export type MaterialCategory = "slides" | "homework" | "other";

export interface Material {
  id: number;
  courseId: number;
  sessionId: number | null;
  fileName: string;
  fileType: string;
  sizeBytes: number;
  /** 本地绝对路径 */
  filePath: string;
  confidence: "high" | "low" | "manual";
  category: MaterialCategory;
  createdAt: string;
}

/** 待办事项（行动项「加入」后或手动创建） */
export interface Todo {
  id: number;
  sessionId: number | null;
  actionItemId: number | null;
  /** ai=行动项加入 / manual=手动添加 */
  source: "ai" | "manual";
  title: string;
  detail: string | null;
  /** RFC3339 或模型给出的原始日期描述；null=DDL 未知 */
  ddl: string | null;
  done: boolean;
  createdAt: string;
  completedAt: string | null;
}

/** 单次模型调用的用量明细 */
export interface UsageRecord {
  id: number;
  sessionId: number | null;
  ts: string;
  /** asr / translate / detect / note / ocr */
  stage: string;
  model: string;
  modelTier: ModelTier | "local";
  inputTokens: number;
  cachedTokens: number;
  outputTokens: number;
  audioSeconds: number;
  amount: number;
}

export interface AppSettings {
  /** 第一周周一，YYYY-MM-DD */
  semesterStart: string;
  monthlyBudget: number;
  speakLanguage: string;
  dailyLanguage: string;
  presenceIntervalSec: number;
  /** 是否用摄像头人脸检测判定在位（关闭时仅支持手动模拟验证） */
  presenceCameraEnabled: boolean;
  /** 课程表可见时间范围（24 小时制整点），默认 8–18 */
  scheduleStartHour: number;
  scheduleEndHour: number;
  /** 麦克风软件增益（0.5–4.0，仅作用于麦克风路径） */
  micGain: number;
  /** 同桌小轩人设 JSON（人设矩阵档位 + 自定义补充），null=全部默认 */
  persona: string | null;
}

/** 同桌小轩人设矩阵（各维度三档，null/缺失=中档默认） */
export interface PersonaMatrix {
  /** 详略：low 简洁 / mid 均衡 / high 细致 */
  detail?: "low" | "mid" | "high";
  /** 语气：cold 高冷 / mid 随和 / warm 热情 */
  warmth?: "cold" | "mid" | "warm";
  /** 风格：serious 严肃 / mid 适中 / fun 幽默 */
  humor?: "serious" | "mid" | "fun";
  /** 自定义系统提示词补充（自由编辑） */
  extra?: string;
}

/** 同桌小轩聊天的一轮历史 */
export interface ChatTurn {
  role: "user" | "assistant";
  content: string;
}

/** 同桌小轩聊天范围 */
export type ChatScope = "dashboard" | "course" | "session";

/** 文稿整理稿：一段合并过度分割、去除口误后的课文 */
export interface TidyPassage {
  fromSeq: number;
  toSeq: number;
  en: string;
  zh: string | null;
}

/** 正在播放声音的应用（网课模式「按应用采集」候选） */
export interface AudioApp {
  pid: number;
  /** 进程友好名（exe 文件名，去扩展名） */
  name: string;
  /** 会话是否正在播放声音 */
  active: boolean;
}

/** 密钥/外部服务配置概览（只含掩码，不含明文） */
export interface SecretConfig {
  siliconflowKeySet: boolean;
  siliconflowKeyMasked: string;
  smtpUser: string;
  smtpTo: string;
  smtpAuthSet: boolean;
  volcKeySet: boolean;
  volcKeyMasked: string;
}

/** 局部更新：空字段表示不修改；clear* 为显式清除 */
export interface SecretConfigPatch {
  siliconflowKey?: string;
  smtpUser?: string;
  smtpTo?: string;
  smtpAuthCode?: string;
  clearSiliconflow?: boolean;
  clearSmtp?: boolean;
  volcKey?: string;
  clearVolc?: boolean;
}

/** 在位状态：present 在位 / away 不在位 / unavailable 检测不可用 */
export type PresenceState = "present" | "away" | "unavailable";

export interface PresenceEvent {
  ts: string;
  state: PresenceState;
}

/** 后端推送的在位状态变化事件 */
export interface PresenceChangedEvent {
  state: PresenceState;
  sessionId: number;
  ts: string;
}

/** 节次时段定义 */
export interface Period {
  label: string;
  startMinute: number;
  endMinute: number;
}

/* ----------------------------- 实时字幕/行动项 ---------------------------- */

/** 一条双语音频字幕行（seq 为 10 秒分块序号） */
export interface TranscriptLine {
  ts: string;
  seq: number;
  en: string;
  zh: string | null;
  /** 说话人：teacher 老师 / other 同学 / "" 未标注 */
  speaker: string;
}

/** 检测出的行动项 */
export interface ActionItem {
  id: number;
  sessionId: number;
  ts: string;
  /** quiz / interaction / deadline / other */
  itemType: string;
  content: string;
  actionGuide: string | null;
  urgency: "high" | "medium" | "low" | null;
  addedToTasks: boolean;
  /** 截止时间：RFC3339 或模型给出的原始日期描述，无则 null */
  ddl: string | null;
}

/** 后端推送：新字幕行 */
export interface SubtitleLineEvent {
  sessionId: number;
  seq: number;
  ts: string;
  en: string;
  zh: string | null;
  speaker: string;
}

/** 后端推送：AST 流式部分字幕（未定稿、边说边出字，定稿后由 SubtitleLineEvent 取代） */
export interface SubtitlePartialEvent {
  sessionId: number;
  en: string;
  zh: string | null;
}

/** 后端推送：字幕通道状态提示（如转写连续失败暂停） */
export interface SubtitleStatusEvent {
  sessionId: number;
  message: string;
}

/** 后端推送：新行动项 */
export interface ActionItemEvent {
  sessionId: number;
  item: ActionItem;
}

/** 小轩请求把对话内容记入笔记（需用户在聊天栏确认） */
export interface NoteSaveRequestEvent {
  requestId: string;
  sessionId: number;
  title: string;
  content: string;
}

/** 小轩流式聊天事件 */
export type ChatStreamEvent =
  | { type: "status"; stage: string; tool?: string }
  | { type: "delta"; text: string }
  | ({ type: "noteRequest" } & NoteSaveRequestEvent);

/** 提示词模板（设置页可编辑） */
export interface PromptTemplate {
  /** tidy / translate_batch / detect / detect_batch / note / chat */
  task: string;
  label: string;
  template: string;
  isDefault: boolean;
}

/** 大模型调用日志条目 */
export interface LlmLog {
  id: number;
  ts: string;
  sessionId: number | null;
  stage: string;
  model: string;
  systemPrompt: string | null;
  userPrompt: string | null;
  response: string | null;
  ok: boolean;
  error: string | null;
  inputTokens: number;
  cachedTokens: number;
  outputTokens: number;
}
