import type { ModelTier, SessionMode } from "../types";

export interface TierOption {
  id: ModelTier;
  name: string;
  emoji: string;
  /** 豆包 AST 2.0 同传估算每小时花费（元，各档相同：同传模型固定为 Doubao-AST-2.0） */
  astRatePerHour: number;
  /** 非实时经济模式估算每小时花费（元：免费 ASR + GLM-4.5-Air 翻译 + 检测/整理） */
  chunkRatePerHour: number;
  desc: string;
  recommended?: boolean;
}

export const TIER_OPTIONS: TierOption[] = [
  {
    id: "free",
    name: "极致省钱",
    emoji: "🥉",
    astRatePerHour: 4.3,
    chunkRatePerHour: 0.3,
    desc: "SenseVoice ASR + GLM-4.5-Air + 缓存检测，日常课优先",
  },
  {
    id: "eco",
    name: "经济版",
    emoji: "🥈",
    astRatePerHour: 4.3,
    chunkRatePerHour: 0.5,
    desc: "轻量模型为主，行动项检测略有延迟",
  },
  {
    id: "balanced",
    name: "均衡版",
    emoji: "🥇",
    astRatePerHour: 4.3,
    chunkRatePerHour: 0.8,
    desc: "质量与成本平衡，默认推荐",
    recommended: true,
  },
  {
    id: "pro",
    name: "高质量版",
    emoji: "💎",
    astRatePerHour: 4.3,
    chunkRatePerHour: 1.5,
    desc: "检测更稳、笔记更深，重难点课使用",
  },
];

export const MODE_INFO: Record<
  SessionMode,
  { name: string; emoji: string; desc: string }
> = {
  online: {
    name: "网课模式",
    emoji: "💻",
    desc: "捕获电脑系统声音（WASAPI），无需麦克风权限",
  },
  offline: {
    name: "线下课模式",
    emoji: "🎙",
    desc: "使用麦克风收录老师讲课声音",
  },
};
