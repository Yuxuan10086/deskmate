<template>
  <div class="warn-bar card">
    ⚠️ 请佩戴<strong>耳机</strong>再开始监听。若使用扬声器，麦克风会产生尖锐回授啸叫。
  </div>

  <div class="card panel">
    <div class="row-2">
      <div class="field">
        <label>输入设备（麦克风）</label>
        <select v-model="inputId" class="select" :disabled="running">
          <option v-for="d in inputs" :key="d.deviceId" :value="d.deviceId">
            {{ d.label || "未知设备" }}
          </option>
          <option v-if="!inputs.length" value="">（未检测到设备，请先授予麦克风权限）</option>
        </select>
      </div>
      <div class="field">
        <label>输出设备（耳机 / 扬声器）</label>
        <select v-model="outputId" class="select">
          <option v-for="d in outputs" :key="d.deviceId" :value="d.deviceId">
            {{ d.label || "系统默认输出" }}
          </option>
          <option v-if="!outputs.length" value="">（默认输出）</option>
        </select>
      </div>
    </div>

    <div class="field">
      <label>信号处理（桌面端录制为原始信号，建议全部关闭来还原真实效果）</label>
      <div class="toggles">
        <label class="chk"><input type="checkbox" v-model="echoCancel" :disabled="running" /> 回声消除</label>
        <label class="chk"><input type="checkbox" v-model="noiseSuppress" :disabled="running" /> 噪声抑制</label>
        <label class="chk"><input type="checkbox" v-model="autoGain" :disabled="running" /> 自动增益</label>
      </div>
    </div>

    <div class="field">
      <label>监听音量：{{ Math.round(gain * 100) }}%</label>
      <input v-model.number="gain" type="range" min="0" max="2" step="0.05" class="slider" />
    </div>

    <div class="btn-row">
      <button v-if="!running" class="btn" :disabled="!supported || starting" @click="start">
        {{ starting ? "正在打开麦克风…" : "● 开始监听" }}
      </button>
      <button v-else class="btn danger" @click="stop">■ 停止监听</button>
      <span v-if="running" class="live-tag">LIVE</span>
    </div>

    <div v-if="errorMsg" class="err-msg">{{ errorMsg }}</div>
    <div v-if="!supported" class="err-msg">
      当前浏览器不支持 getUserMedia / Web Audio，无法使用本工具。
    </div>
  </div>

  <div v-if="running || lastStats" class="card panel">
    <h3>实时电平</h3>
    <div class="meter-wrap">
      <div class="meter">
        <div class="meter-fill" :style="{ width: meterPct + '%' }"></div>
        <div class="clip-mark"></div>
      </div>
      <div class="meter-scale"><span>静音</span><span>-40dB</span><span>-20dB</span><span>-6dB</span><span>0</span></div>
    </div>

    <div class="stat-grid">
      <div class="stat">
        <div class="stat-label">当前 RMS</div>
        <div class="stat-val">{{ rmsI16 }}</div>
        <div class="stat-sub">{{ rmsDb }} dBFS</div>
      </div>
      <div class="stat">
        <div class="stat-label">峰值</div>
        <div class="stat-val" :class="{ clip: peakClipped }">{{ peakI16 }}</div>
        <div class="stat-sub">{{ peakClipped ? "已削顶！" : "i16 量纲（满幅 32767）" }}</div>
      </div>
      <div class="stat">
        <div class="stat-label">最大 RMS（本次）</div>
        <div class="stat-val">{{ maxRmsI16 }}</div>
        <div class="stat-sub">正常说话通常 500 以上</div>
      </div>
      <div class="stat">
        <div class="stat-label">硬件声道数</div>
        <div class="stat-val" :class="{ warn: (lastStats?.channels ?? 0) > 2 }">
          {{ lastStats?.channels ?? "-" }}
        </div>
        <div class="stat-sub">桌面端会下混为单声道；&gt;2 需留意</div>
      </div>
      <div class="stat">
        <div class="stat-label">硬件采样率</div>
        <div class="stat-val">{{ lastStats?.hwRate ?? "-" }}</div>
        <div class="stat-sub">Hz · 桌面端统一重采样到 16000</div>
      </div>
      <div class="stat">
        <div class="stat-label">链路延迟</div>
        <div class="stat-val">{{ latencyMs }}</div>
        <div class="stat-sub">毫秒（仅供参考）</div>
      </div>
    </div>

    <div class="judge">
      <strong>怎么判断：</strong>戴上耳机后用正常上课音量说话——RMS 应稳定在
      <strong>500 以上</strong>（桌面端静音告警阈值为 200）；轻声也应有 100–300。
      若大声说话 RMS 仍长期低于 200、或声道数/采样率异常，说明该输入设备不适合，
      可在上方切换其他输入设备对比。
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";

interface DevItem {
  deviceId: string;
  label: string;
}

const supported =
  typeof navigator !== "undefined" &&
  !!navigator.mediaDevices &&
  !!navigator.mediaDevices.getUserMedia &&
  typeof window.AudioContext !== "undefined";

const inputs = ref<DevItem[]>([]);
const outputs = ref<DevItem[]>([]);
const inputId = ref("");
const outputId = ref("");

const echoCancel = ref(false);
const noiseSuppress = ref(false);
const autoGain = ref(false);
const gain = ref(1);

const running = ref(false);
const starting = ref(false);
const errorMsg = ref("");
const rmsI16 = ref(0);
const peakI16 = ref(0);
const maxRmsI16 = ref(0);
const peakClipped = ref(false);
const latencyMs = ref("-");

interface Stats {
  channels: number;
  hwRate: number;
}
const lastStats = ref<Stats | null>(null);

let stream: MediaStream | null = null;
let audioCtx: AudioContext | null = null;
let rafId = 0;
let audioEl: HTMLAudioElement | null = null;
let gainNodeRef: GainNode | null = null;
/** 是否为用户主动停止（用于区分轨道意外中断） */
const intentionalClose = ref(false);

async function enumerate() {
  if (!supported) return;
  const list = await navigator.mediaDevices.enumerateDevices();
  inputs.value = list
    .filter((d) => d.kind === "audioinput")
    .map((d) => ({ deviceId: d.deviceId, label: d.label }));
  outputs.value = list
    .filter((d) => d.kind === "audiooutput")
    .map((d) => ({ deviceId: d.deviceId, label: d.label }));
  if (!inputId.value && inputs.value.length) inputId.value = inputs.value[0].deviceId;
  if (!outputId.value) outputId.value = "";
}

async function start() {
  errorMsg.value = "";
  peakClipped.value = false;
  maxRmsI16.value = 0;
  starting.value = true;
  intentionalClose.value = false;
  try {
    // 8 秒超时：设备被独占占用时 getUserMedia 可能一直挂起不返回
    let timedOut = false;
    const timeout = new Promise<never>((_, reject) =>
      setTimeout(
        () => {
          timedOut = true;
          reject(
            new Error(
              "打开麦克风超时（8 秒无响应）。请检查系统「设置 → 隐私 → 麦克风」权限，或关闭正在占用麦克风的程序（会议软件、其他录音机等）后重试"
            )
          );
        },
        8000
      )
    );
    const micPromise = navigator.mediaDevices.getUserMedia({
      audio: {
        deviceId: inputId.value ? { exact: inputId.value } : undefined,
        echoCancellation: echoCancel.value,
        noiseSuppression: noiseSuppress.value,
        autoGainControl: autoGain.value,
      },
      video: false,
    });
    // 仅在“超时已发生、流迟到”时回收，绝不能在正常路径停止轨道
    micPromise
      .then((s) => {
        if (timedOut) s.getTracks().forEach((t) => t.stop());
      })
      .catch(() => undefined);
    stream = await Promise.race([micPromise, timeout]);
    await enumerate(); // 授权后拿到真实设备名

    audioCtx = new AudioContext();
    // 无用户手势（如自动启动）时上下文可能是 suspended
    if (audioCtx.state === "suspended") {
      audioCtx.resume().catch(() => undefined);
    }
    const source = audioCtx.createMediaStreamSource(stream);
    const gainNode = audioCtx.createGain();
    gainNode.gain.value = gain.value;
    gainNodeRef = gainNode;
    const analyser = audioCtx.createAnalyser();
    analyser.fftSize = 2048;
    // 下混单声道后输出，与桌面端采集的下混行为一致
    const dest = audioCtx.createMediaStreamDestination();
    dest.channelCount = 1;
    source.connect(analyser);
    source.connect(gainNode);
    gainNode.connect(dest);

    audioEl = new Audio();
    audioEl.srcObject = dest.stream;
    if (outputId.value && typeof (audioEl as any).setSinkId === "function") {
      await (audioEl as any).setSinkId(outputId.value);
    }
    // play() 可能被浏览器自动播放策略拦截：此时麦克风与电平表仍正常工作，
    // 只提示用户点击页面恢复耳返，绝不能因此停掉整条链路。
    try {
      await audioEl.play();
    } catch {
      errorMsg.value =
        "麦克风已开启（任务栏应出现麦克风图标），但浏览器拦截了自动播放。点击页面任意位置即可在耳机中听到声音。";
      const resumePlay = () => {
        audioCtx?.resume().catch(() => undefined);
        audioEl?.play().catch(() => undefined);
        errorMsg.value = "";
        window.removeEventListener("click", resumePlay);
      };
      window.addEventListener("click", resumePlay, { once: true });
    }

    const track = stream.getAudioTracks()[0];
    // 设备被拔出/驱动重置/被系统中断时，给出明确反馈而不是一直显示 LIVE
    track.addEventListener("ended", () => {
      if (!intentionalClose.value) {
        errorMsg.value =
          "麦克风轨道被意外中断（设备可能被拔出、被系统重置，或被其他程序抢占），请重新开始监听。";
        running.value = false;
        starting.value = false;
        rmsI16.value = 0;
        peakI16.value = 0;
      }
    });
    const st = track.getSettings();
    lastStats.value = {
      channels: st.channelCount ?? 1,
      hwRate: st.sampleRate ?? audioCtx.sampleRate,
    };
    latencyMs.value = String(
      Math.round(((audioCtx.baseLatency ?? 0) + ((audioCtx as any).outputLatency ?? 0)) * 1000)
    );

    running.value = true;
    starting.value = false;
    loopMeter(analyser);
  } catch (e: any) {
    starting.value = false;
    errorMsg.value = `无法启动：${e?.message ?? e}`;
    cleanup();
  }
}

function loopMeter(analyser: AnalyserNode) {
  const buf = new Float32Array(analyser.fftSize);
  const tick = () => {
    analyser.getFloatTimeDomainData(buf);
    let sum = 0;
    let peak = 0;
    for (let i = 0; i < buf.length; i++) {
      const v = buf[i];
      sum += v * v;
      const a = Math.abs(v);
      if (a > peak) peak = a;
    }
    const rms = Math.sqrt(sum / buf.length);
    rmsI16.value = Math.round(rms * 32767);
    peakI16.value = Math.round(peak * 32767);
    if (rmsI16.value > maxRmsI16.value) maxRmsI16.value = rmsI16.value;
    if (peak >= 0.999) peakClipped.value = true;
    rafId = requestAnimationFrame(tick);
  };
  tick();
}

function stop() {
  intentionalClose.value = true;
  cleanup();
  running.value = false;
  starting.value = false;
  rmsI16.value = 0;
  peakI16.value = 0;
}

function cleanup() {
  cancelAnimationFrame(rafId);
  stream?.getTracks().forEach((t) => t.stop());
  stream = null;
  if (audioEl) {
    audioEl.pause();
    audioEl.srcObject = null;
    audioEl = null;
  }
  if (audioCtx) {
    void audioCtx.close();
    audioCtx = null;
  }
  gainNodeRef = null;
}

/** 输出设备热切换（不中断监听） */
async function applyOutput() {
  if (audioEl && outputId.value && typeof (audioEl as any).setSinkId === "function") {
    try {
      await (audioEl as any).setSinkId(outputId.value);
    } catch (e: any) {
      errorMsg.value = `切换输出失败：${e?.message ?? e}`;
    }
  }
}

watch(outputId, () => void applyOutput());
watch(gain, (v) => {
  if (audioCtx && gainNodeRef) {
    gainNodeRef.gain.setTargetAtTime(v, audioCtx.currentTime, 0.01);
  }
});

const rmsDb = computed(() => {
  if (rmsI16.value <= 0) return "-∞";
  return (20 * Math.log10(rmsI16.value / 32767)).toFixed(1);
});
const meterPct = computed(() => {
  // 以 dBFS 映射：-60dB → 0%，0dB → 100%
  if (rmsI16.value <= 0) return 0;
  const db = 20 * Math.log10(rmsI16.value / 32767);
  return Math.min(100, Math.max(0, ((db + 60) / 60) * 100));
});

onMounted(() => {
  void enumerate();
  navigator.mediaDevices?.addEventListener("devicechange", () => void enumerate());
});
onBeforeUnmount(cleanup);
</script>

<style scoped>
.warn-bar {
  padding: 12px 18px;
  margin-bottom: 14px;
  font-size: 13px;
  color: #9a3412;
  background: #fff7ed;
  border: 1px solid #fed7aa;
}
.panel {
  padding: 20px 22px;
  margin-bottom: 16px;
}
.panel h3 {
  margin: 0 0 14px;
  font-size: 14px;
}
.row-2 {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0 16px;
}
.field {
  margin-bottom: 16px;
}
.field label {
  display: block;
  font-size: 13px;
  color: var(--ink-soft);
  margin-bottom: 6px;
}
.toggles {
  display: flex;
  gap: 20px;
}
.chk {
  font-size: 13px;
  color: var(--ink-soft);
  display: flex;
  align-items: center;
  gap: 6px;
}
.slider {
  width: 100%;
  max-width: 420px;
}
.btn-row {
  display: flex;
  align-items: center;
  gap: 12px;
}
.btn.danger {
  color: #fff;
  background: var(--danger);
  border-color: var(--danger);
}
.live-tag {
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 1px;
  color: #e23b3b;
  animation: blink 1.2s infinite;
}
@keyframes blink {
  50% {
    opacity: 0.3;
  }
}
.err-msg {
  margin-top: 12px;
  font-size: 13px;
  color: #c2410c;
}
.meter-wrap {
  margin-bottom: 18px;
}
.meter {
  position: relative;
  height: 22px;
  border-radius: 6px;
  background: #eef1f6;
  overflow: hidden;
}
.meter-fill {
  height: 100%;
  background: linear-gradient(90deg, #22c55e 0%, #eab308 70%, #ef4444 100%);
  transition: width 0.05s linear;
}
.clip-mark {
  position: absolute;
  top: 0;
  bottom: 0;
  left: 90%;
  width: 1px;
  background: rgba(0, 0, 0, 0.25);
}
.meter-scale {
  display: flex;
  justify-content: space-between;
  font-size: 10px;
  color: var(--ink-mute);
  margin-top: 3px;
}
.stat-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 12px;
  margin-bottom: 16px;
}
.stat {
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 10px 14px;
  background: #fafbfe;
}
.stat-label {
  font-size: 11px;
  color: var(--ink-mute);
}
.stat-val {
  font-size: 22px;
  font-weight: 700;
  color: var(--ink);
  font-variant-numeric: tabular-nums;
}
.stat-val.clip {
  color: #e23b3b;
}
.stat-val.warn {
  color: #d97706;
}
.stat-sub {
  font-size: 10px;
  color: var(--ink-mute);
}
.judge {
  font-size: 12px;
  line-height: 1.7;
  color: var(--ink-soft);
  background: var(--brand-soft);
  border-radius: 8px;
  padding: 10px 14px;
}
</style>
