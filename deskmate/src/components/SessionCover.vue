<template>
  <div class="cover" :style="{ background: bg }" title="点击下载封面" @click="exportPng">
    <div class="cv-top">
      <span class="cv-course">{{ courseName }}</span>
      <span class="cv-mode">{{ mode === "online" ? "网课" : "线下" }}</span>
    </div>
    <div class="cv-lecture">第 {{ lectureNo }} 讲</div>
    <div class="cv-bottom">
      <span>{{ dateText }}</span>
      <span>{{ timeText }}</span>
    </div>
    <div class="cv-download" title="下载 PNG 封面">⬇ 导出封面</div>
  </div>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { minutesToHM } from "../utils/date";
import type { SessionMode } from "../types";

const props = defineProps<{
  courseName: string;
  color: string;
  lectureNo: number;
  dateText: string;
  startMinute: number;
  endMinute: number;
  mode: SessionMode;
}>();

const bg = computed(
  () =>
    `linear-gradient(135deg, ${props.color} 0%, ${shade(props.color, -28)} 100%)`
);
const timeText = computed(
  () => `${minutesToHM(props.startMinute)}–${minutesToHM(props.endMinute)}`
);

/** 简单加深/提亮十六进制颜色 */
function shade(hex: string, percent: number): string {
  const n = parseInt(hex.replace("#", ""), 16);
  const amt = Math.round(2.55 * percent);
  const r = Math.min(255, Math.max(0, (n >> 16) + amt));
  const g = Math.min(255, Math.max(0, ((n >> 8) & 0x00ff) + amt));
  const b = Math.min(255, Math.max(0, (n & 0x0000ff) + amt));
  return `#${((r << 16) | (g << 8) | b).toString(16).padStart(6, "0")}`;
}

/** Canvas 绘制封面并触发 PNG 下载 */
function exportPng() {
  const W = 1920;
  const H = 1080;
  const cv = document.createElement("canvas");
  cv.width = W;
  cv.height = H;
  const ctx = cv.getContext("2d");
  if (!ctx) return;

  // 135° 渐变背景（模拟 CSS linear-gradient）
  const grad = ctx.createLinearGradient(0, 0, W, H);
  grad.addColorStop(0, props.color);
  grad.addColorStop(1, shade(props.color, -28));
  ctx.fillStyle = grad;
  ctx.fillRect(0, 0, W, H);

  // 右上角装饰圆
  ctx.beginPath();
  ctx.fillStyle = "rgba(255,255,255,0.1)";
  ctx.arc(W - 260, -160, 320, 0, Math.PI * 2);
  ctx.fill();

  // 顶部课程名
  ctx.fillStyle = "rgba(255,255,255,0.92)";
  ctx.font = "500 36px 'Microsoft YaHei', 'PingFang SC', sans-serif";
  ctx.textBaseline = "top";
  ctx.fillText(props.courseName, 70, 55);

  // 模式徽标
  const modeText = props.mode === "online" ? "网课" : "线下";
  ctx.font = "500 24px 'Microsoft YaHei', sans-serif";
  const modeW = ctx.measureText(modeText).width + 40;
  const modeX = W - 70 - modeW;
  ctx.fillStyle = "rgba(255,255,255,0.22)";
  roundRect(ctx, modeX, 60, modeW, 44, 22);
  ctx.fill();
  ctx.fillStyle = "#fff";
  ctx.textAlign = "center";
  ctx.fillText(modeText, modeX + modeW / 2, 70);
  ctx.textAlign = "start";

  // 中央讲次（大号）
  ctx.fillStyle = "#fff";
  ctx.font = "700 180px 'Microsoft YaHei', 'PingFang SC', sans-serif";
  ctx.textBaseline = "middle";
  ctx.shadowColor = "rgba(0,0,0,0.15)";
  ctx.shadowBlur = 8;
  ctx.fillText(`第 ${props.lectureNo} 讲`, 70, H / 2);
  ctx.shadowBlur = 0;

  // 底部日期 + 时间
  const dateStr = props.dateText;
  const timeStr = timeText.value;
  ctx.font = "400 32px 'Microsoft YaHei', sans-serif";
  ctx.textBaseline = "bottom";
  ctx.fillStyle = "rgba(255,255,255,0.92)";
  ctx.fillText(dateStr, 70, H - 60);
  ctx.textAlign = "right";
  ctx.fillText(timeStr, W - 70, H - 60);
  ctx.textAlign = "start";

  // 下载
  const url = cv.toDataURL("image/png");
  const a = document.createElement("a");
  a.href = url;
  const safe = props.courseName.replace(/[\\/:*?"<>|]/g, "_");
  a.download = `${safe}_第${props.lectureNo}讲.png`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

defineExpose({ exportPng });
</script>

<style scoped>
.cover {
  width: 100%;
  aspect-ratio: 16 / 9;
  border-radius: 10px;
  color: #fff;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  position: relative;
  overflow: hidden;
  cursor: pointer;
}
.cover::after {
  content: "";
  position: absolute;
  right: -30px;
  top: -30px;
  width: 110px;
  height: 110px;
  border-radius: 50%;
  background: rgba(255, 255, 255, 0.1);
}
.cv-download {
  position: absolute;
  right: 10px;
  bottom: 10px;
  font-size: 11px;
  background: rgba(0, 0, 0, 0.22);
  padding: 4px 10px;
  border-radius: 10px;
  opacity: 0;
  transition: opacity 0.2s;
}
.cover:hover .cv-download {
  opacity: 1;
}
.cv-top {
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-size: 11px;
  opacity: 0.92;
}
.cv-course {
  max-width: 70%;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cv-mode {
  background: rgba(255, 255, 255, 0.22);
  padding: 1px 8px;
  border-radius: 8px;
  font-size: 10px;
}
.cv-lecture {
  font-size: 22px;
  font-weight: 700;
  letter-spacing: 1px;
  text-shadow: 0 1px 4px rgba(0, 0, 0, 0.15);
}
.cv-bottom {
  display: flex;
  justify-content: space-between;
  font-size: 11px;
  opacity: 0.92;
}
</style>
