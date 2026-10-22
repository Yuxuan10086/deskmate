<template>
  <div class="page session-page" v-if="session">
    <input ref="pptInput" type="file" multiple hidden @change="onFilesPicked" />

    <!-- 附件分类选择：上传前逐个指定 课件PPT / 作业题目 / 其他附件 -->
    <div v-if="pendingUploads.length" class="popup-mask">
      <div class="popup-card upload-card">
        <div class="popup-content">这些文件归类为？</div>
        <div class="up-list">
          <div v-for="(u, i) in pendingUploads" :key="i" class="up-row">
            <span class="up-name" :title="u.file.name">📄 {{ u.file.name }}</span>
            <select v-model="u.category" class="up-cat">
              <option value="slides">课件 PPT</option>
              <option value="homework">作业题目</option>
              <option value="other">其他附件</option>
            </select>
          </div>
        </div>
        <div class="popup-actions">
          <button class="btn ghost sm" @click="cancelUpload">取消</button>
          <button class="btn sm" :disabled="uploading" @click="confirmUpload">
            {{ uploading ? `上传中 ${uploadDone}/${pendingUploads.length}` : `上传 ${pendingUploads.length} 个文件` }}
          </button>
        </div>
      </div>
    </div>
    <div class="page-head">
      <button class="btn ghost sm" @click="$router.back()">← 返回</button>
      <h1>{{ session.title }}</h1>
      <span class="badge" :class="session.status === 'recording' ? 'orange' : 'green'">
        {{ session.status === "recording" ? "● 录制中" : "已归档" }}
      </span>
    </div>

    <!-- 录制中面板 -->
    <div v-if="session.status === 'recording'" class="card rec-panel">
      <div class="rec-pulse"><span></span></div>
      <div class="rec-info">
        <div class="rec-title">正在记录 · {{ course?.name ?? "自由记录" }}</div>
        <div class="rec-timer">{{ elapsedText }}</div>
        <div class="rec-sub">
          {{ MODE_INFO[session.mode].name }} · {{ tierName }} · 按真实 token 计费
        </div>
        <div class="rec-sub">
          在位检测：<b :class="presenceClass">{{ presenceText }}</b>
          <span v-if="presenceState === 'unavailable' && presenceDetail" class="presence-detail">
            （{{ presenceDetail }}）
          </span>
        </div>
        <div class="rec-sub mic-line">
          <span class="mic-name">🎙 {{ capDevice || "正在打开采集设备…" }}</span>
          <span class="level-bar"><i :style="{ width: levelPct + '%' }"></i></span>
          <b v-if="silenceWarn" class="mic-warn">
            ⚠ 未检测到声音，请检查麦克风设备 / 系统权限 / 输入音量
          </b>
        </div>
        <!-- 采集异常提示：如按应用采集拿不到声音已自动回退系统声音 -->
        <div v-if="capNote" class="rec-sub cap-note">⚠ {{ capNote }}</div>
        <!-- 麦克风增益：拖动实时生效，采集线程按采样直接乘增益（网课模式录系统声音，增益不适用） -->
        <div v-if="session.mode !== 'online'" class="rec-sub tool-row">
          <span class="tool-label">🎚 麦克风增益</span>
          <input
            v-model.number="micGain"
            class="gain-slider"
            type="range"
            min="0.5"
            max="4"
            step="0.1"
            @input="onGainInput"
          />
          <b class="gain-val">×{{ micGain.toFixed(1) }}</b>
        </div>
        <!-- 网课模式：按应用采集（只录网课，不受其他声音干扰）+ 接管课堂 -->
        <template v-if="session.mode === 'online'">
          <div class="rec-sub tool-row">
            <span class="tool-label">🎧 采集声音来源</span>
            <select v-model="selectedPid" class="app-select" @change="onAppSelected">
              <option :value="null">系统全部声音</option>
              <option v-for="a in audioApps" :key="a.pid" :value="a.pid">
                {{ a.name }}{{ a.active ? " · 正在播放" : "" }}
              </option>
            </select>
            <button class="btn ghost sm" :disabled="appsLoading" @click="refreshAudioApps">
              {{ appsLoading ? "…" : "↻ 刷新" }}
            </button>
          </div>
          <div class="rec-sub tool-row">
            <span class="tool-label">📢 接管课堂</span>
            <button
              class="btn sm"
              :class="{ ghost: takenOver }"
              :disabled="selectedPid == null || takingOver"
              @click="toggleTakeOver"
            >
              {{ takenOver ? "✓ 已接管 · 点击恢复外放" : "静音本机外放（只录音不外放）" }}
            </button>
            <span v-if="selectedPid == null" class="tool-hint">先选择网课应用</span>
            <span v-else class="tool-hint">接管期间整台电脑静音，录音与字幕正常</span>
          </div>
        </template>
        <!-- 课堂进行中也可上传本次附件：归档时会结合课件并自动生成笔记与行动项 -->
        <div class="rec-sub tool-row">
          <span class="tool-label">📎 本次附件</span>
          <button class="btn ghost sm" @click="uploadPpt">⬆ 上传附件</button>
          <span v-if="sessionMaterials.length" class="tool-hint">
            已传 {{ sessionMaterials.length }} 个：{{ sessionMaterials.map((m) => m.fileName).join("、") }}
          </span>
          <span v-else class="tool-hint">课件 / 作业题 / 任意资料均可，上传时选择分类</span>
        </div>
      </div>
      <button class="btn danger" :disabled="finishing" @click="finish">
        {{ finishing ? "归档中…" : "■ 结束课程并归档" }}
      </button>
    </div>

    <!-- 已归档：封面 + 基本信息 -->
    <template v-else>
      <div class="overview">
        <div class="cover-wrap">
          <SessionCover
            :course-name="course?.name ?? '自由记录'"
            :color="course?.color ?? '#64748b'"
            :lecture-no="session.lectureNo"
            :date-text="coverDate"
            :start-minute="session.startMinute"
            :end-minute="session.endMinute"
            :mode="session.mode"
          />
        </div>
        <div class="card meta-card">
          <div class="meta-row"><span>课程</span><b>{{ course?.name ?? "未关联" }}</b></div>
          <div class="meta-row"><span>时间</span><b>{{ fullTimeText }}</b></div>
          <div class="meta-row"><span>节次</span><b>第 {{ session.weekNo }} 周 · {{ WEEKDAY_NAMES[session.weekday] }}</b></div>
          <div class="meta-row"><span>地点</span><b>{{ session.location || "—" }}</b></div>
          <div class="meta-row"><span>模式</span><b>{{ MODE_INFO[session.mode].name }}</b></div>
          <div class="meta-row"><span>模型系列</span><b>{{ tierName }}</b></div>
          <div class="meta-row"><span>时长</span><b>{{ formatDuration(session.durationSec) }}</b></div>
          <div class="meta-row cost"><span>实际花费</span><b>¥{{ session.costEstimate.toFixed(2) }}</b></div>
        </div>
      </div>

      <div class="resume-bar">
        <button class="btn sm" @click="resumeRecording">● 继续录制（接着原有录音与字幕）</button>
        <button class="btn ghost sm" :disabled="openingDir" @click="openAudioDir">
          📁 打开原音频
        </button>
        <span class="flex-spacer"></span>
        <button class="btn danger sm" :disabled="deleting" @click="deleteSession">
          {{ deleting ? "删除中…" : "🗑 删除课堂" }}
        </button>
      </div>

      <div class="tabs">
        <button
          v-for="t in tabs"
          :key="t.id"
          class="tab-btn"
          :class="{ active: tab === t.id }"
          @click="tab = t.id"
        >
          {{ t.label }}
        </button>
      </div>

      <div class="card tab-panel">
        <!-- 附件：按分类分组，可打开 / 删除 -->
        <div v-if="tab === 'materials'">
          <div class="panel-head">
            <h3>附件</h3>
            <button class="btn ghost sm" @click="uploadPpt">⬆ 上传附件</button>
          </div>
          <div v-if="sessionMaterials.length === 0" class="mini-empty">
            尚未上传附件。支持任意类型文件，上传时选择「课件 PPT / 作业题目 / 其他附件」。
          </div>
          <template v-for="g in materialGroups" :key="g.key">
            <div v-if="g.items.length" class="mat-group">
              <div class="mat-group-title">{{ g.label }}（{{ g.items.length }}）</div>
              <div v-for="m in g.items" :key="m.id" class="mat-row">
                <span>📄</span>
                <span class="mat-name" :title="m.filePath">{{ m.fileName }}</span>
                <span class="mat-size">{{ formatSize(m.sizeBytes) }}</span>
                <button class="btn ghost sm" @click="openMaterialFile(m.id)">打开</button>
                <button class="btn ghost sm danger-text" @click="removeMaterial(m.id)">删除</button>
              </div>
            </div>
          </template>
        </div>

        <!-- 双语文本：优先展示整理版（合并过度分割、去除口误），可切回原始字幕 -->
        <div v-else-if="tab === 'transcript'">
          <div class="panel-head">
            <h3>双语文本</h3>
            <span v-if="tidyPassages.length" class="tidy-switch">
              <button class="btn sm" :class="{ ghost: !tidyView }" @click="tidyView = true">
                整理版
              </button>
              <button class="btn sm" :class="{ ghost: tidyView }" @click="tidyView = false">
                原始字幕
              </button>
            </span>
            <span v-if="tidyPassages.length && tidyView" class="tidy-hint">
              已合并被过度分割的语句并去除口误
            </span>
          </div>
          <template v-if="tidyView && tidyPassages.length">
            <div v-for="(p, i) in tidyPassages" :key="'p' + i" class="ts-row">
              <div class="ts-en">{{ p.en }}</div>
              <div class="ts-zh">{{ p.zh ?? "（未翻译）" }}</div>
            </div>
          </template>
          <template v-else>
            <div v-if="transcriptLines.length === 0" class="mini-empty">
              本节课无转写文本（录制时未配置 API Key 或未捕获到语音）。
            </div>
            <div v-if="hasOtherSpeaker" class="speaker-legend">
              带【同学】前缀的内容不是老师讲的，是课堂里其他同学的发言
            </div>
            <div v-for="line in transcriptLines" :key="line.seq" class="ts-row">
              <div class="ts-time">[{{ line.seq }}] {{ timeOf(line.ts) }}</div>
              <div class="ts-en" :class="{ 'spk-other': line.speaker === 'other' }">
                {{ line.speaker === "other" ? "【同学】" : "" }}{{ line.en }}
              </div>
              <div class="ts-zh" :class="{ 'spk-other': line.speaker === 'other' }">
                {{ line.speaker === "other" ? "【同学】" : "" }}{{ line.zh ?? "（未翻译）" }}
              </div>
            </div>
            <!-- 录制中的流式未定稿行 -->
            <div v-if="partialLine" class="ts-row partial-row">
              <div class="ts-time">…</div>
              <div class="ts-en">{{ partialLine.en }}…</div>
              <div class="ts-zh">{{ partialLine.zh ?? "…" }}</div>
            </div>
          </template>
        </div>

        <!-- 笔记 -->
        <div v-else-if="tab === 'note'">
          <div class="panel-head">
            <h3>课堂笔记</h3>
            <button class="btn sm" :disabled="regenerating" @click="regenNote">
              {{ regenerating ? "生成中…" : noteText ? "↻ 重新生成" : "✨ 生成笔记" }}
            </button>
          </div>
          <pre v-if="noteText" class="note-text">{{ noteText }}</pre>
          <div v-else class="mini-empty">
            本节课没有自动生成笔记（课堂时长不足 3 分钟或实质内容过少）；如确有需要，可点击右上角手动生成。
          </div>
        </div>

        <!-- 任务 -->
        <div v-else-if="tab === 'tasks'">
          <div class="panel-head"><h3>任务 / 项目清单</h3></div>
          <div v-if="actionItems.length === 0" class="mini-empty">
            本节课未捕获到行动项（课堂中出现测验、互动或截止提醒时会自动出现在这里）。
          </div>
          <div v-for="it in actionItems" :key="it.id" class="task-row">
            <span class="badge" :class="it.urgency === 'high' ? 'orange' : 'green'">
              {{ typeName(it.itemType) }}
            </span>
            <div class="task-main">
              <div class="task-content">{{ it.content }}</div>
              <div v-if="it.actionGuide" class="task-guide">操作指引：{{ it.actionGuide }}</div>
              <div class="task-ddl">
                <label class="ddl-pick">
                  📅 {{ ddlText(it.ddl) }}
                  <input
                    type="date"
                    class="ddl-picker"
                    :value="ddlInputValue(it.ddl)"
                    @change="changeItemDdl(it, ($event.target as HTMLInputElement).value)"
                  />
                </label>
              </div>
            </div>
            <button v-if="!it.addedToTasks" class="btn ghost sm" @click="addTask(it)">
              加入待办
            </button>
            <template v-else>
              <span class="badge green">已加入</span>
              <button class="btn ghost sm unjoin" title="从待办中移除" @click="removeTask(it)">
                取消加入
              </button>
            </template>
          </div>
        </div>

        <!-- 花费明细：默认按环节聚合，点击展开逐条明细 -->
        <div v-else>
          <div class="panel-head">
            <h3>本次花费明细</h3>
            <span class="usage-total">合计 <b>¥{{ usageTotal.toFixed(3) }}</b></span>
          </div>
          <table class="usage-tbl">
            <thead>
              <tr><th style="width:34px"></th><th>环节</th><th>模型</th><th>用量</th><th>金额</th></tr>
            </thead>
            <tbody>
              <template v-for="g in usageGroups" :key="g.stage">
                <tr class="usage-group" @click="toggleGroup(g.stage)">
                  <td class="usage-caret">{{ expanded[g.stage] ? "▾" : "▸" }}</td>
                  <td>{{ stageName(g.stage) }} <span class="usage-count">{{ g.count }} 条</span></td>
                  <td>{{ g.models }}</td>
                  <td class="usage-amt">{{ groupDetail(g) }}</td>
                  <td>{{ g.amount > 0 ? `¥${g.amount.toFixed(3)}` : "免费" }}</td>
                </tr>
                <template v-if="expanded[g.stage]">
                  <tr v-for="u in g.items" :key="u.id" class="usage-item">
                    <td></td>
                    <td class="usage-time">{{ timeOf(u.ts) }}</td>
                    <td>{{ u.model }}</td>
                    <td class="usage-amt">{{ usageDetail(u) }}</td>
                    <td>{{ u.amount > 0 ? `¥${u.amount.toFixed(3)}` : "免费" }}</td>
                  </tr>
                </template>
              </template>
            </tbody>
          </table>
          <div v-if="usageList.length === 0" class="mini-empty">暂无用量明细。</div>
        </div>
      </div>
    </template>

    <!-- 实时双语字幕（录制中，PRD 5.4）：滚动容器，可上滑回看历史字幕 -->
    <div v-if="session.status === 'recording'" class="card live-sub">
      <div class="live-head">
        <span class="live-dot"></span>
        <span class="live-title">实时双语字幕</span>
        <!-- 录制中实时切换：同传 / 经济转写（每 10 秒）/ 关闭 -->
        <div class="engine-switch" title="同传约 ¥4.3/小时；经济模式免费转写+低价翻译约 ¥0.3~1.5/小时">
          <button
            class="es-btn"
            :class="{ on: engine === 'ast' }"
            :disabled="engineSwitching"
            @click="switchEngine('ast')"
          >同传</button>
          <button
            class="es-btn"
            :class="{ on: engine === 'cheap' }"
            :disabled="engineSwitching"
            @click="switchEngine('cheap')"
          >经济</button>
          <button
            class="es-btn"
            :class="{ on: engine === 'off' }"
            :disabled="engineSwitching"
            @click="switchEngine('off')"
          >关</button>
        </div>
        <span v-if="liveStatus" class="live-status">{{ liveStatus }}</span>
        <button v-if="!liveStick" class="btn ghost sm live-latest" @click="scrollLive(true)">
          ↓ 回到最新
        </button>
      </div>
      <div ref="liveBody" class="live-body" @scroll="onLiveScroll">
        <template v-if="liveLines.length > 0">
          <div
            v-for="(l, i) in liveLines"
            :key="l.seq"
            class="ts-row"
            :class="{ 'live-row': i === liveLines.length - 1 }"
          >
            <div class="ts-time">[{{ l.seq }}] {{ timeOf(l.ts) }}</div>
            <div class="ts-en" :class="{ 'spk-other': l.speaker === 'other' }">
              {{ l.speaker === "other" ? "【同学】" : "" }}{{ l.en }}
            </div>
            <div class="ts-zh" :class="{ 'spk-other': l.speaker === 'other' }">
              {{ l.speaker === "other" ? "【同学】" : "" }}{{ l.zh ?? "（未翻译）" }}
            </div>
          </div>
        </template>
        <!-- AST 流式部分字幕：未定稿，边说边出字 -->
        <div v-if="partialLine" class="ts-row partial-row">
          <div class="ts-en">{{ partialLine.en }}…</div>
          <div class="ts-zh">{{ partialLine.zh ?? "…" }}</div>
        </div>
        <div v-else-if="liveLines.length === 0 && !liveStatus" class="mini-empty">
          正在监听课堂声音，识别到讲话后字幕将实时显示在这里。
        </div>
      </div>
    </div>

    <!-- 行动项弹窗（录制中实时检测，PRD 5.5） -->
    <div v-if="popupItem" class="popup-mask">
      <div class="popup-card">
        <div class="popup-tag" :class="popupItem.urgency === 'high' ? 'high' : ''">
          {{ typeName(popupItem.itemType) }} · {{ urgencyName(popupItem.urgency) }}优先级
        </div>
        <div class="popup-content">{{ popupItem.content }}</div>
        <div v-if="popupItem.actionGuide" class="popup-guide">建议：{{ popupItem.actionGuide }}</div>
        <div class="popup-actions">
          <button class="btn ghost sm" @click="snoozePopup">稍后提醒</button>
          <button class="btn ghost sm" @click="addPopupToTasks">加入课后任务</button>
          <button class="btn sm" @click="dismissPopup">知道了</button>
        </div>
      </div>
    </div>
  </div>
  <div v-else class="page empty">课堂不存在。</div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { useDataStore } from "../stores/data";
import SessionCover from "../components/SessionCover.vue";
import { MODE_INFO, TIER_OPTIONS } from "../utils/tiers";
import { WEEKDAY_NAMES, formatDuration } from "../utils/date";
import type { ActionItem, AudioApp, Material, MaterialCategory, PresenceState, SubtitlePartialEvent, TidyPassage, TranscriptLine, UsageRecord } from "../types";
import {
  api,
  isTauri,
  onPresenceChanged,
  onSubtitleLine,
  onSubtitlePartial,
  onSubtitleStatus,
  onActionItemNew,
} from "../api/ipc";
import { errMsg } from "../api/ipc";
import { notify } from "../utils/notify";

const route = useRoute();
const router = useRouter();
const store = useDataStore();

const session = computed(() =>
  store.sessions.find((s) => s.id === Number(route.params.id))
);
const course = computed(() =>
  session.value ? store.courseById(session.value.courseId) : undefined
);
const tier = computed(() =>
  TIER_OPTIONS.find((t) => t.id === session.value?.modelTier)
);
const tierName = computed(() => tier.value?.name ?? "—");

const tabs = [
  { id: "usage", label: "花费明细" },
  { id: "materials", label: "附件" },
  { id: "transcript", label: "双语文本" },
  { id: "note", label: "课堂笔记" },
  { id: "tasks", label: "任务清单" },
] as const;
const tab = ref<(typeof tabs)[number]["id"]>("usage");

const finishing = ref(false);
const elapsedText = ref("00:00");
let timer: ReturnType<typeof setInterval> | undefined;

/** 本次录制段的开始时间戳（继续录制时重置，归档时长按本段计算由后端累加） */
const recStartAt = ref(Date.now());
/** 已点「结束并归档」：计时冻结在点击时刻（归档管线耗时不再计入） */
const recordingEnded = ref(false);

/* ------------------------------ 采集自检 ------------------------------ */
const capDevice = ref("");
const capRms = ref(0);
const silenceWarn = ref(false);
/** 采集异常提示（如按应用采集拿不到声音已自动回退系统声音），由后端 capture_status 提供 */
const capNote = ref<string | null>(null);
let capTimer: ReturnType<typeof setInterval> | undefined;
/** RMS 满量程按 6000 映射电平条（正常说话 RMS 1k~8k） */
const levelPct = computed(() => Math.min(100, Math.round((capRms.value / 6000) * 100)));

/* --------------------- 麦克风增益 / 按应用采集 / 接管课堂 --------------------- */
const micGain = ref(1.0);
let gainTimer: ReturnType<typeof setTimeout> | undefined;
/** 拖动增益滑块（250ms 防抖后实时下发后端并持久化）。 */
function onGainInput() {
  clearTimeout(gainTimer);
  gainTimer = setTimeout(async () => {
    gainTimer = undefined;
    try {
      micGain.value = await api.setMicGain(micGain.value);
    } catch (e) {
      console.warn("[mic] 增益设置失败", e);
    }
  }, 250);
}

const audioApps = ref<AudioApp[]>([]);
const appsLoading = ref(false);
/** null = 采集系统全部声音；数字 = 仅采集该 pid 应用（进程回环） */
const selectedPid = ref<number | null>(null);
const takenOver = ref(false);
const takingOver = ref(false);
/** 正在切换采集目标（期间轮询不回写选择，避免下拉框闪回） */
let switchingTarget = false;

async function refreshAudioApps() {
  appsLoading.value = true;
  try {
    const list = await api.listAudioApps();
    audioApps.value = list;
    // 当前选择的目标已退出时回落到系统声音
    if (selectedPid.value != null && !list.some((a) => a.pid === selectedPid.value)) {
      selectedPid.value = null;
    }
  } catch (e) {
    console.warn("[audio] 应用列表获取失败", e);
  }
  appsLoading.value = false;
}

/** 切换采集目标：重启采集线程；若此前已接管则自动把静音迁移到新目标。 */
async function onAppSelected() {
  if (!session.value || switchingTarget) return;
  const sid = session.value.id;
  const mode = session.value.mode === "online" ? "online" : "offline";
  switchingTarget = true;
  try {
    if (takenOver.value) {
      await api.releaseClass();
      takenOver.value = false;
    }
    await api.startCapture(sid, mode, selectedPid.value);
    if (selectedPid.value != null) {
      await api.takeOverClass(selectedPid.value);
      takenOver.value = true;
    }
  } catch (e) {
    alert(`切换采集目标失败：${errMsg(e)}`);
  }
  switchingTarget = false;
}

/** 接管课堂：端点级静音整机外放（回环录音不受影响），释放时恢复原状。 */
async function toggleTakeOver() {
  if (selectedPid.value == null || takingOver.value) return;
  takingOver.value = true;
  try {
    if (!takenOver.value) {
      await api.takeOverClass(selectedPid.value);
      takenOver.value = true;
    } else {
      await api.releaseClass();
      takenOver.value = false;
    }
  } catch (e) {
    alert(`接管操作失败：${errMsg(e)}`);
  }
  takingOver.value = false;
}

/* --------------------------- 文稿整理（双语文本 tab） --------------------------- */
const tidyPassages = ref<TidyPassage[]>([]);
const tidyView = ref(true);
/** 录制中定时刷新整理稿（后端整理任务 60s 一轮，前端同步跟一次） */
let tidyTimer: ReturnType<typeof setInterval> | undefined;

/** 拉取整理稿（录制中由后端后台任务定时整理，归档后兜底补齐）。 */
async function loadTidy(sid: number) {
  try {
    tidyPassages.value = await api.listTidyPassages(sid);
  } catch (e) {
    console.warn("[tidy] 整理稿加载失败", e);
  }
}

/* ------------------------------ 在位检测 ------------------------------ */
const presenceState = ref<PresenceState | null>(null);
/** 摄像头不可用时的原因（如被占用/隐私设置阻止），仅当状态为 unavailable 时展示 */
const presenceDetail = ref<string | null>(null);
let unlistenPresence: (() => void) | undefined;

/** 拉取一次在位状态与不可用原因（启动、续录与状态变化时统一走这里）。 */
async function syncPresenceStatus() {
  try {
    const st = await api.presenceStatus();
    presenceState.value = st.state;
    presenceDetail.value = st.detail;
  } catch {
    /* 忽略单次快照失败 */
  }
}

/** 待处理行动项（不在位时触发邮件兜底的判据；由实时检测填充） */
const pendingItems = ref<ActionItem[]>([]);

/* ------------------------- 实时字幕/行动项（PRD 5.4/5.5） ------------------------- */
const liveLines = ref<TranscriptLine[]>([]);
const liveStatus = ref("");
/** 当前字幕引擎（ast 同传 / cheap 经济转写 / off 关闭），可录制中热切换 */
const engine = ref<"ast" | "cheap" | "off">("ast");
const engineSwitching = ref(false);
/** AST 流式部分字幕（未定稿行），null 表示当前无进行中的 utterance */
const partialLine = ref<SubtitlePartialEvent | null>(null);
const transcriptLines = ref<TranscriptLine[]>([]);
const actionItems = ref<ActionItem[]>([]);
const popupItem = ref<ActionItem | null>(null);
const regenerating = ref(false);
let unlistenSub: (() => void) | undefined;
let unlistenSubPartial: (() => void) | undefined;
let unlistenSubStatus: (() => void) | undefined;
let unlistenItem: (() => void) | undefined;
let snoozeTimer: ReturnType<typeof setTimeout> | undefined;

/* 实时字幕滚动：默认贴底跟随最新；用户上滑回看历史时不强制拉回 */
const liveBody = ref<HTMLElement | null>(null);
const liveStick = ref(true);

function onLiveScroll() {
  const el = liveBody.value;
  if (!el) return;
  liveStick.value = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
}
function scrollLive(force = false) {
  const el = liveBody.value;
  if (el && (liveStick.value || force)) el.scrollTop = el.scrollHeight;
}
watch(
  () => [liveLines.value.length, partialLine.value?.en],
  async () => {
    await nextTick();
    scrollLive();
  }
);

function typeName(t: string): string {
  return (
    {
      quiz: "随堂测验",
      interaction: "课堂互动",
      deadline: "截止提醒",
      homework: "课后作业",
      submission: "提交任务",
      reading: "阅读/预习",
      exam: "测验考试",
      material: "材料准备",
      other: "其他",
    }[t] ?? "其他"
  );
}

/* ------------------------------- DDL 编辑 ------------------------------- */
function ddlInputValue(v: string | null): string {
  if (!v) return "";
  const d = new Date(v);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")}`;
}
function ddlText(v: string | null): string {
  if (!v) return "DDL 未知 · 点击设置";
  const d = new Date(v);
  if (Number.isNaN(d.getTime())) return `${v} · 点击改为日期`;
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const day = new Date(d.getFullYear(), d.getMonth(), d.getDate());
  const diff = Math.round((day.getTime() - today.getTime()) / 86400000);
  const base = `${d.getMonth() + 1}月${d.getDate()}日`;
  if (diff === 0) return "今天截止";
  if (diff === 1) return "明天截止";
  if (diff < 0) return `${base}（已过期）`;
  return `${base}（${diff} 天后）`;
}
async function changeItemDdl(it: ActionItem, ymd: string) {
  try {
    await api.setActionItemDdl(it.id, ymd);
    it.ddl = ymd || null;
    actionItems.value = [...actionItems.value];
    await store.reloadTodos();
  } catch (e) {
    alert(`修改 DDL 失败：${errMsg(e)}`);
  }
}
function urgencyName(u: string | null): string {
  return { high: "高", medium: "中", low: "低" }[u ?? ""] ?? "中";
}

/** Tauri 命令失败抛的是字符串，统一提取可读错误文本（见 api/ipc.errMsg）。 */

async function addTask(it: ActionItem) {
  try {
    await api.markActionItem(it.id);
    it.addedToTasks = true;
    actionItems.value = [...actionItems.value];
    await store.reloadTodos();
  } catch (e) {
    alert(`操作失败：${errMsg(e)}`);
  }
}

/** 取消加入：删除对应待办并复位行动项标记。 */
async function removeTask(it: ActionItem) {
  try {
    await api.unmarkActionItem(it.id);
    it.addedToTasks = false;
    actionItems.value = [...actionItems.value];
    await store.reloadTodos();
  } catch (e) {
    alert(`操作失败：${errMsg(e)}`);
  }
}
function dismissPopup() {
  clearTimeout(snoozeTimer);
  popupItem.value = null;
}
function snoozePopup() {
  const item = popupItem.value;
  popupItem.value = null;
  if (item) {
    snoozeTimer = setTimeout(() => {
      popupItem.value = item;
    }, 5 * 60 * 1000);
  }
}
async function addPopupToTasks() {
  const item = popupItem.value;
  if (!item) return;
  await addTask(item);
  popupItem.value = null;
}

/** 用课件与字幕重新生成课堂笔记（PRD 5.8）。 */
async function regenNote() {
  if (!session.value || regenerating.value) return;
  regenerating.value = true;
  try {
    await api.regenerateNote(session.value.id);
    await store.reloadSessions();
    await store.reloadUsage();
    await notify("笔记已更新", "已用课件与双语文本重新生成课堂笔记");
  } catch (e) {
    alert(`重新生成失败：${errMsg(e)}`);
  }
  regenerating.value = false;
}

const presenceText = computed(() => {
  switch (presenceState.value) {
    case "away":
      return "不在位";
    case "unavailable":
      return "检测不可用";
    case "present":
      return "在位";
    default:
      return "—";
  }
});
const presenceClass = computed(() =>
  presenceState.value === "away" ? "presence-away" : ""
);

/**
 * 不在位：桌面弹窗始终触发，同时发一封邮件，让你知道「同桌知道你离开了」（PRD 5.6）。
 * 邮件未配置时后端静默跳过，不影响弹窗主通道。
 */
async function onPresenceState(next: PresenceState) {
  if (next !== "away") return;
  const items = pendingItems.value;
  await notify(
    "你已离开座位",
    items.length > 0
      ? `课堂仍在录制，还有 ${items.length} 条待处理行动项`
      : "课堂仍在录制"
  );

  const now = new Date();
  const hhmm = `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}`;
  const title = session.value?.title ?? "当前课堂";
  const courseName = course.value?.name;
  const body = [
    `${hhmm} 检测到你已离开座位，${courseName ? `${courseName} · ` : ""}课堂「${title}」仍在录制中。`,
    "",
    items.length > 0
      ? `当前有 ${items.length} 条待处理行动项：\n${items
          .map((it, i) => `${i + 1}. ${it.content}`)
          .join("\n")}`
      : "当前没有待处理的行动项。",
    "",
    "回到座位后录制会继续；下课时会自动生成课堂笔记与行动项。",
    "",
    "—— 同桌小轩",
  ].join("\n");
  try {
    await api.sendReminderEmail("同桌小轩 · 已检测到你离开座位", body);
  } catch (e) {
    console.warn("[presence] 邮件发送失败", e);
  }
}

const materials = ref<Material[]>([]);
const pptInput = ref<HTMLInputElement | null>(null);

const usageList = computed<UsageRecord[]>(() =>
  store.usage
    .filter((u) => u.sessionId === session.value?.id)
    .sort((a, b) => a.ts.localeCompare(b.ts))
);

interface UsageGroup {
  stage: string;
  count: number;
  amount: number;
  inputTokens: number;
  cachedTokens: number;
  outputTokens: number;
  audioSeconds: number;
  models: string;
  items: UsageRecord[];
}

const usageGroups = computed<UsageGroup[]>(() => {
  const map = new Map<string, UsageGroup>();
  for (const u of usageList.value) {
    let g = map.get(u.stage);
    if (!g) {
      g = {
        stage: u.stage,
        count: 0,
        amount: 0,
        inputTokens: 0,
        cachedTokens: 0,
        outputTokens: 0,
        audioSeconds: 0,
        models: "",
        items: [],
      };
      map.set(u.stage, g);
    }
    g.count += 1;
    g.amount += u.amount;
    g.inputTokens += u.inputTokens;
    g.cachedTokens += u.cachedTokens;
    g.outputTokens += u.outputTokens;
    g.audioSeconds += u.audioSeconds;
    g.items.push(u);
  }
  const groups = [...map.values()];
  for (const g of groups) {
    g.models = [...new Set(g.items.map((i) => i.model))].join("、");
  }
  // 金额大的环节排前面
  return groups.sort((a, b) => b.amount - a.amount);
});

const usageTotal = computed(() =>
  usageGroups.value.reduce((s, g) => s + g.amount, 0)
);

// 默认全部收起
const expanded = reactive<Record<string, boolean>>({});
function toggleGroup(stage: string) {
  expanded[stage] = !expanded[stage];
}
function groupDetail(g: UsageGroup): string {
  if (g.audioSeconds > 0) return `${g.audioSeconds} 秒音频`;
  const parts: string[] = [];
  if (g.inputTokens) parts.push(`入 ${g.inputTokens}`);
  if (g.cachedTokens) parts.push(`缓存 ${g.cachedTokens}`);
  if (g.outputTokens) parts.push(`出 ${g.outputTokens}`);
  return parts.join(" / ") || "—";
}

const coverDate = computed(() => {
  if (!session.value) return "";
  const d = new Date(session.value.startAt);
  return `${d.getMonth() + 1}/${String(d.getDate()).padStart(2, "0")}`;
});
const fullTimeText = computed(() => {
  if (!session.value) return "";
  const d = new Date(session.value.startAt);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(
    d.getDate()
  ).padStart(2, "0")} ${WEEKDAY_NAMES[session.value.weekday]}`;
});

const noteText = computed(() => session.value?.note ?? "");

function timeOf(iso: string): string {
  const d = new Date(iso);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}
function stageName(s: string): string {
  return (
    {
      asr: "ASR 转写",
      ast: "同声传译",
      translate: "字幕翻译",
      tidy: "文稿整理",
      detect: "行动项检测",
      speaker: "说话人标注",
      archive: "归档（笔记+行动项）",
      note: "笔记生成",
      chat: "同桌对话",
      ocr: "课件 OCR",
    }[s] ?? s
  );
}
function usageDetail(u: UsageRecord): string {
  if (u.audioSeconds > 0) return `${u.audioSeconds} 秒音频`;
  const parts: string[] = [];
  if (u.inputTokens) parts.push(`入 ${u.inputTokens}`);
  if (u.cachedTokens) parts.push(`缓存 ${u.cachedTokens}`);
  if (u.outputTokens) parts.push(`出 ${u.outputTokens}`);
  return parts.join(" / ") || "—";
}

async function loadMaterials() {
  // 拉取课程级全量附件（含课程公共附件 sessionId=null），本课堂的由 sessionMaterials 过滤
  if (session.value?.courseId != null) {
    materials.value = await api.listMaterials(session.value.courseId);
  }
}

function tick() {
  if (!session.value) return;
  // 已点击「结束并归档」：计时停在点击时刻，不再随归档耗时继续走
  if (recordingEnded.value) return;
  // 累计时长 = 历次归档时长 + 本次录制段（续录场景 startAt 已过时，不能直接用它）
  const historical = session.value.durationSec ?? 0;
  const sec = Math.max(0, historical + Math.floor((Date.now() - recStartAt.value) / 1000));
  const hh = String(Math.floor(sec / 3600)).padStart(2, "0");
  const mm = String(Math.floor((sec % 3600) / 60)).padStart(2, "0");
  const ss = String(sec % 60).padStart(2, "0");
  elapsedText.value = `${hh}:${mm}:${ss}`;
}

async function finish() {
  if (!session.value) return;
  finishing.value = true;
  // 只上报「本段」时长，后端在历史 duration_sec 上累加
  const durationSec = Math.max(1, Math.floor((Date.now() - recStartAt.value) / 1000));
  recordingEnded.value = true;
  // 停止系统音频采集、在位检测与实时字幕
  clearInterval(capTimer);
  try {
    await api.stopCapture();
    await api.stopPresence();
    await api.stopLive();
    console.log("[capture] stopped");
  } catch (e) {
    console.warn("[capture] stop failed", e);
  }
  // 花费完全由后端按真实用量汇总，前端不再预估
  try {
    await api.finishSession(session.value.id, { durationSec, costEstimate: 0 });
  } catch (e) {
    // 归档失败：课堂仍为录制中，恢复计时与按钮，避免界面卡在「归档中…」
    recordingEnded.value = false;
    finishing.value = false;
    await notify("归档失败", errMsg(e));
    return;
  }
  await store.reloadSessions();
  await store.reloadUsage();
  // 就地切到归档态：onMounted 的归档分支不会重跑，必须手动加载已落库的双语文本
  // 与行动项——否则第一次打开（录制→归档的连续会话）双语文本显示为空
  const sid = session.value.id;
  try {
    transcriptLines.value = await api.listTranscript(sid);
    actionItems.value = await api.listActionItems(sid);
  } catch (e) {
    console.warn("[detail] 归档后转写/行动项加载失败", e);
  }
  // 行动项的 DDL 在归档时自动识别，刷新全局待办（已「加入待办」的条目拿到 DDL）
  await store.reloadTodos();
  // 归档兜底整理稿已生成，切到整理版视图
  await loadTidy(sid);
  tidyView.value = true;
  liveLines.value = [];
  partialLine.value = null;
  finishing.value = false;
  await notify(
    "课堂已归档",
    "双语文本、课堂笔记、课后行动项与花费明细均已自动生成，可在各标签页查看"
  );
}

/** 归档后继续录制：状态重开后重启采集/在位/字幕，分块与字幕序号后端自动续接。 */
async function resumeRecording() {
  if (!session.value) return;
  const sid = session.value.id;
  await api.resumeSession(sid);
  await store.reloadSessions();
  recStartAt.value = Date.now();
  recordingEnded.value = false;
  // 关键：续录前页面处于归档态，onMounted 未订阅实时事件，必须在此补订阅
  await ensureLiveSubscriptions(sid);
  await seedLiveLines(sid);
  await startRecordingServices(sid);
  tick();
}

/** 打开本节课录音分块所在目录（10 秒一个 WAV 文件）。 */
const openingDir = ref(false);
async function openAudioDir() {
  if (!session.value) return;
  openingDir.value = true;
  try {
    const path = await api.openCapturesDir(session.value.id);
    if (!isTauri()) alert(`浏览器环境无法打开本地目录。\n原音频目录：${path}`);
  } catch (e) {
    alert(`打开失败：${errMsg(e)}`);
  } finally {
    openingDir.value = false;
  }
}

/** 删除课堂（软删除 → 设置页回收站可恢复/彻底删除）。 */
const deleting = ref(false);
async function deleteSession() {
  if (!session.value) return;
  if (!confirm(`将「${session.value.title}」移入回收站？可在设置页恢复或彻底删除。`)) return;
  deleting.value = true;
  try {
    await api.softDeleteSession(session.value.id);
    // 同步移除全局 store，课程表/课程列表立刻不再显示该课堂
    store.sessions = store.sessions.filter((s) => s.id !== session.value!.id);
    router.back();
  } catch (e) {
    alert(`删除失败：${errMsg(e)}`);
  } finally {
    deleting.value = false;
  }
}

/* --------------------------- 附件批量上传（手动分类） --------------------------- */
interface PendingUpload {
  file: File;
  category: MaterialCategory;
}
const pendingUploads = ref<PendingUpload[]>([]);
const uploading = ref(false);
const uploadDone = ref(0);

function uploadPpt() {
  pptInput.value?.click();
}

/** 根据扩展名猜测默认分类：演示/文档类默认课件 PPT，其余默认其他附件（用户可逐个改）。 */
function guessCategory(name: string): MaterialCategory {
  return /\.(pptx?|pdf|key|keyx)$/i.test(name) ? "slides" : "other";
}

/** 选完文件先入队并弹出分类选择卡，确认后才真正上传。 */
function onFilesPicked(e: Event) {
  const input = e.target as HTMLInputElement;
  if (!input.files || input.files.length === 0) return;
  for (const f of Array.from(input.files)) {
    pendingUploads.value.push({ file: f, category: guessCategory(f.name) });
  }
  input.value = "";
}
function cancelUpload() {
  if (uploading.value) return;
  pendingUploads.value = [];
}
function fileToBase64(f: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(String(r.result).split(",").pop() ?? "");
    r.onerror = () => reject(r.error);
    r.readAsDataURL(f);
  });
}
async function confirmUpload() {
  if (!session.value || uploading.value) return;
  uploading.value = true;
  uploadDone.value = 0;
  const failed: string[] = [];
  for (const u of pendingUploads.value) {
    try {
      const b64 = await fileToBase64(u.file);
      await api.uploadMaterial({
        fileName: u.file.name,
        base64: b64,
        courseId: session.value.courseId ?? 0,
        sessionId: session.value.id,
        category: u.category,
      });
    } catch (err) {
      failed.push(`${u.file.name}（${errMsg(err)}）`);
    }
    uploadDone.value += 1;
  }
  pendingUploads.value = [];
  uploading.value = false;
  await loadMaterials();
  if (failed.length) {
    alert(`部分文件上传失败：\n${failed.join("\n")}`);
  }
}

/** 仅本课堂的附件（materials 为课程级全量列表）。 */
const sessionMaterials = computed<Material[]>(() =>
  materials.value.filter((m) => m.sessionId === session.value?.id)
);
/** 附件按分类分组展示：课件 PPT / 作业题目 / 其他附件。 */
const materialGroups = computed(() => {
  const defs: { key: MaterialCategory; label: string }[] = [
    { key: "slides", label: "课件 PPT" },
    { key: "homework", label: "作业题目" },
    { key: "other", label: "其他附件" },
  ];
  return defs.map((d) => ({
    ...d,
    items: sessionMaterials.value.filter((m) => m.category === d.key),
  }));
});
function formatSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}
async function openMaterialFile(id: number) {
  try {
    const path = await api.openMaterial(id);
    if (!isTauri()) alert(`浏览器环境无法打开本地文件。\n路径：${path}`);
  } catch (e) {
    alert(`打开失败：${errMsg(e)}`);
  }
}
async function removeMaterial(id: number) {
  if (!confirm("确定删除该附件？文件会从本机目录一并移除。")) return;
  try {
    await api.deleteMaterial(id);
    await loadMaterials();
  } catch (e) {
    alert(`删除失败：${errMsg(e)}`);
  }
}

/** 是否存在非老师发言（有则在双语文本顶部展示记号说明）。 */
const hasOtherSpeaker = computed(() =>
  transcriptLines.value.some((l) => l.speaker === "other")
);

/** 轮询采集指标：设备名、实时电平、启动期静音告警；顺带同步采集/接管目标。 */
let appPollCount = 0;
/** 采集线程意外退出的自动重启次数（第一次用当前所选来源重试，第二次退回系统声音） */
let captureRestartCount = 0;
/** 自动重启进行中：200ms 轮询与重启调用（后端可能耗时数百毫秒）并发时会重复触发重启 */
let restartingCapture = false;
async function pollCapture() {
  try {
    const st = await api.captureStatus();
    capDevice.value = st.deviceName;
    capRms.value = st.rms;
    silenceWarn.value = st.silenceWarned;
    capNote.value = st.captureNote ?? null;
    engine.value = st.engine ?? "off";
    if (!switchingTarget) {
      // 与后端保持一致（页面重进时恢复应用选择与接管状态）
      selectedPid.value = st.targetPid ?? null;
      takenOver.value = st.mutedPid != null;
    }
    // 采集线程意外退出（如按应用采集初始化失败、设备被拔出）：自动重启。
    // 第一次按当前所选来源重试；再退出则退回系统声音（一定可用）。
    if (
      session.value?.status === "recording" &&
      st.sessionId === session.value.id &&
      !st.running &&
      !switchingTarget &&
      !restartingCapture &&
      !finishing.value
    ) {
      if (captureRestartCount < 2) {
        captureRestartCount++;
        const pid = captureRestartCount === 1 ? selectedPid.value : null;
        capNote.value = "音频采集意外停止，正在自动重启…";
        restartingCapture = true;
        try {
          await api.startCapture(
            session.value.id,
            session.value.mode === "online" ? "online" : "offline",
            pid
          );
        } catch {
          /* 下轮轮询再试 */
        } finally {
          restartingCapture = false;
        }
      } else if (captureRestartCount === 2) {
        captureRestartCount = 3;
        capNote.value = "音频采集多次意外停止，请手动切换采集来源，或结束课程后重新开始";
      }
    } else if (st.running) {
      captureRestartCount = 0;
    }
    // 网课模式每 10 秒刷新一次应用列表（200ms × 50，新开的应用能被及时选中）
    if (session.value?.mode === "online" && ++appPollCount % 50 === 0) {
      void refreshAudioApps();
    }
  } catch {
    /* 忽略单次轮询失败 */
  }
}

/** 录制中热切换字幕引擎。 */
async function switchEngine(target: "ast" | "cheap" | "off") {
  if (engine.value === target || engineSwitching.value || !session.value) return;
  engineSwitching.value = true;
  try {
    const actual = await api.setLiveEngine(session.value.id, target);
    engine.value = actual;
    if (actual === "cheap") {
      partialLine.value = null;
      liveStatus.value = "已切换经济模式：每 10 秒更新字幕，语句会自动拼接完整";
    } else if (actual === "ast") {
      liveStatus.value = "已切换为豆包同声传译";
    } else {
      partialLine.value = null;
      liveStatus.value = "字幕已关闭，录音仍在继续";
    }
  } catch (e) {
    alert(`切换失败：${errMsg(e)}`);
  } finally {
    engineSwitching.value = false;
  }
}

/** 启动/续录共用：采集（按课堂模式）+ 在位检测 + 实时字幕。 */
async function startRecordingServices(sid: number) {
  const mode = session.value?.mode === "online" ? "online" : "offline";
  try {
    await api.startCapture(sid, mode);
    console.log("[capture] started for session", sid, mode);
  } catch (e) {
    console.warn("[capture] start failed (可能无麦克风权限)", e);
  }
  try {
    unlistenPresence = await onPresenceChanged((e) => {
      if (e.sessionId !== sid) return;
      presenceState.value = e.state;
      // 变为不可用时补拉一次详情，拿到失败原因供用户排查
      if (e.state === "unavailable") void syncPresenceStatus();
      void onPresenceState(e.state);
    });
    await api.startPresence(sid);
    await syncPresenceStatus();
  } catch (e) {
    console.warn("[presence] start failed", e);
  }
  // 实时字幕引擎已随 startCapture 按已配置密钥自动启动（有火山 Key 开同传，否则开经济转写），
  // 引擎状态由 pollCapture 轮询同步；用户可在字幕面板头部实时切换
  try {
    const items = await api.listActionItems(sid);
    pendingItems.value = items.filter((it) => !it.addedToTasks);
  } catch {
    /* mock 环境无此数据 */
  }
  clearInterval(capTimer);
  void pollCapture();
  // 200ms 轮询：电平条跟随真实音量跳动；capture_status 仅读内存指标，开销可忽略
  capTimer = setInterval(() => void pollCapture(), 200);
  // 网课模式：拉取可选声音来源应用列表
  if (mode === "online") void refreshAudioApps();
}

/* 实时字幕/行动项事件订阅（幂等：整页生命周期只订阅一次）。 */
let liveSubscribed = false;
async function ensureLiveSubscriptions(sid: number) {
  if (liveSubscribed) return;
  liveSubscribed = true;
  try {
    // 先订阅字幕/行动项事件，再启动采集，避免漏掉首条事件
    unlistenSub = await onSubtitleLine((e) => {
      if (e.sessionId !== sid) return;
      partialLine.value = null;
      const merged = liveLines.value.filter((l) => l.seq !== e.seq);
      merged.push({ ts: e.ts, seq: e.seq, en: e.en, zh: e.zh, speaker: e.speaker });
      merged.sort((a, b) => a.seq - b.seq);
      liveLines.value = merged.slice(-60);
    });
    unlistenSubPartial = await onSubtitlePartial((e) => {
      if (e.sessionId !== sid) return;
      // en 为空 = 定稿清空信号；否则更新打字机行
      partialLine.value = e.en.trim() ? e : null;
    });
    unlistenSubStatus = await onSubtitleStatus((e) => {
      if (e.sessionId === sid) liveStatus.value = e.message;
    });
    unlistenItem = await onActionItemNew((e) => {
      if (e.sessionId !== sid) return;
      pendingItems.value.push(e.item);
      popupItem.value = e.item;
    });
  } catch (e) {
    liveSubscribed = false;
    console.warn("[live] 事件订阅失败", e);
  }
}

/** 从数据库补齐已落库的历史字幕（页面重进/续录时不丢已有内容）。 */
async function seedLiveLines(sid: number) {
  try {
    liveLines.value = await api.listTranscript(sid);
  } catch (e) {
    console.warn("[live] 历史字幕加载失败", e);
  }
}

onMounted(async () => {
  await store.loadAll();
  await loadMaterials();
  timer = setInterval(tick, 1000);
  tick();
  // 恢复上次的麦克风增益设置
  try {
    const s = await api.getSettings();
    micGain.value = s.micGain ?? 1.0;
  } catch {
    /* mock/默认值 */
  }
  // 录制中的课堂自动启动系统音频采集、在位检测与实时字幕
  if (session.value?.status === "recording") {
    const sid = session.value.id;
    await ensureLiveSubscriptions(sid);
    await seedLiveLines(sid);
    await loadTidy(sid);
    tidyTimer = setInterval(() => loadTidy(sid), 60_000);
    await startRecordingServices(sid);
  } else if (session.value) {
    // 已归档：加载真实转写与行动项
    const sid = session.value.id;
    try {
      transcriptLines.value = await api.listTranscript(sid);
      actionItems.value = await api.listActionItems(sid);
      await loadTidy(sid);
    } catch (e) {
      console.warn("[detail] 转写/行动项加载失败", e);
    }
  }
});
onBeforeUnmount(() => {
  clearInterval(timer);
  clearInterval(capTimer);
  clearInterval(tidyTimer);
  clearTimeout(snoozeTimer);
  // 未下发的增益调整立即落库（仅当存在未完成的防抖时）
  if (gainTimer !== undefined) {
    clearTimeout(gainTimer);
    gainTimer = undefined;
    void api.setMicGain(micGain.value).catch(() => undefined);
  }
  unlistenPresence?.();
  unlistenSub?.();
  unlistenSubPartial?.();
  unlistenSubStatus?.();
  unlistenItem?.();
  void api.stopPresence().catch(() => undefined);
  void api.stopLive().catch(() => undefined);
});

void router;
</script>

<style scoped>
.session-page {
  /* 最小宽度：录制面板/字幕区过窄时改由外层出横向滚动条；不能太宽，否则窄窗口+右侧聊天栏时大片空白 */
  min-width: 600px;
}
.overview {
  display: grid;
  grid-template-columns: 320px 1fr;
  gap: 16px;
  margin-bottom: 20px;
}
.meta-card {
  padding: 18px 22px;
}
.presence-away {
  color: #c2410c;
}
/* 摄像头不可用原因（灰色小字，附带在状态后） */
.presence-detail {
  color: var(--muted);
  font-size: 12px;
}
.meta-row {
  display: flex;
  justify-content: space-between;
  padding: 7px 0;
  font-size: 13px;
  border-bottom: 1px dashed var(--line);
}
.meta-row:last-child {
  border-bottom: none;
}
.meta-row span {
  color: var(--ink-mute);
}
.meta-row.cost b {
  color: var(--brand);
  font-size: 16px;
}
.rec-panel {
  display: flex;
  align-items: center;
  gap: 20px;
  padding: 28px;
  margin-bottom: 20px;
}
.rec-pulse {
  width: 54px;
  height: 54px;
  border-radius: 50%;
  background: rgba(239, 68, 68, 0.12);
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
}
.rec-pulse span {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: var(--danger);
  animation: pulse 1.4s infinite;
}
@keyframes pulse {
  0% { box-shadow: 0 0 0 0 rgba(239, 68, 68, 0.45); }
  70% { box-shadow: 0 0 0 16px rgba(239, 68, 68, 0); }
  100% { box-shadow: 0 0 0 0 rgba(239, 68, 68, 0); }
}
.rec-info {
  flex: 1;
}
.rec-title {
  font-size: 15px;
  font-weight: 600;
}
.rec-timer {
  font-size: 30px;
  font-weight: 700;
  font-variant-numeric: tabular-nums;
  letter-spacing: 1px;
}
.rec-sub {
  font-size: 12px;
  color: var(--ink-mute);
}
.tabs {
  display: flex;
  gap: 6px;
  margin-bottom: 12px;
}
.tab-btn {
  border: none;
  background: transparent;
  padding: 8px 16px;
  border-radius: 8px;
  cursor: pointer;
  font-size: 13px;
  color: var(--ink-soft);
}
.tab-btn:hover {
  background: #eef1f7;
}
.tab-btn.active {
  background: var(--brand);
  color: #fff;
  font-weight: 600;
}
.tab-panel {
  padding: 20px 24px;
  min-height: 200px;
}
.panel-head {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 14px;
}
.panel-head h3 {
  margin: 0;
  font-size: 14px;
}
.panel-head .btn {
  margin-left: auto;
}
.mini-empty {
  font-size: 13px;
  color: var(--ink-mute);
  padding: 20px 4px;
}
.mat-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 10px 4px;
  border-bottom: 1px solid var(--line);
  font-size: 13px;
}
.mat-name {
  flex: 1;
}
.ts-row {
  padding: 10px 0;
  border-bottom: 1px dashed var(--line);
}
.ts-en {
  font-size: 14px;
  color: var(--ink);
}
.ts-zh {
  font-size: 13px;
  color: var(--ink-mute);
  margin-top: 2px;
}
.note-text {
  font-family: inherit;
  white-space: pre-wrap;
  font-size: 13px;
  line-height: 1.9;
  color: var(--ink-soft);
  margin: 0;
}
.usage-tbl {
  width: 100%;
  border-collapse: collapse;
  font-size: 12px;
  box-shadow: none;
}
.usage-tbl th {
  text-align: left;
  background: #f4f6fb;
  padding: 8px 10px;
  font-size: 11px;
  color: var(--ink-soft);
}
.usage-tbl td {
  padding: 7px 10px;
  border-bottom: 1px solid var(--line);
}
.usage-amt {
  color: var(--ink-mute);
}
.usage-total {
  font-size: 13px;
  color: var(--ink-mute);
}
.usage-total b {
  color: var(--brand);
  font-size: 16px;
}
.usage-group {
  cursor: pointer;
  font-weight: 600;
  background: #fafbfe;
}
.usage-group:hover {
  background: #f0f4ff;
}
.usage-caret {
  color: var(--ink-mute);
  text-align: center;
}
.usage-count {
  font-weight: 400;
  font-size: 11px;
  color: var(--ink-mute);
  margin-left: 6px;
}
.usage-item td {
  font-size: 11px;
  color: var(--ink-mute);
  background: #fff;
}
.usage-item .usage-time {
  white-space: nowrap;
}

/* 实时字幕 */
.live-sub {
  padding: 16px 22px;
  margin-bottom: 20px;
}
.live-head {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 10px;
  font-size: 13px;
  font-weight: 600;
}
.live-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--danger);
  animation: pulse 1.4s infinite;
}
.live-title {
  color: var(--ink);
}
.engine-switch {
  display: inline-flex;
  border: 1px solid var(--line);
  border-radius: 8px;
  overflow: hidden;
  margin-left: 4px;
}
.es-btn {
  border: none;
  background: #fff;
  color: var(--ink-mute);
  font-size: 12px;
  padding: 3px 10px;
  cursor: pointer;
}
.es-btn + .es-btn {
  border-left: 1px solid var(--line);
}
.es-btn.on {
  background: var(--brand);
  color: #fff;
  font-weight: 600;
}
.es-btn:disabled {
  opacity: 0.6;
  cursor: wait;
}
.live-status {
  font-size: 12px;
  color: var(--ink-mute);
  font-weight: 400;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.live-row .ts-en {
  color: var(--brand);
  font-weight: 600;
}
/* AST 流式部分字幕：未定稿行，弱化显示 */
.partial-row {
  opacity: 0.55;
}
.partial-row .ts-en {
  color: var(--ink-mute);
  font-style: italic;
}
.ts-time {
  font-size: 11px;
  color: var(--ink-mute);
  margin-bottom: 2px;
}

/* 行动项弹窗 */
.popup-mask {
  position: fixed;
  inset: 0;
  background: rgba(15, 23, 42, 0.35);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 60;
}
.popup-card {
  width: 380px;
  background: #fff;
  border-radius: 14px;
  padding: 20px 22px;
  box-shadow: 0 12px 40px rgba(15, 23, 42, 0.25);
}
.popup-tag {
  display: inline-block;
  font-size: 11px;
  padding: 3px 10px;
  border-radius: 999px;
  background: #eef1f7;
  color: var(--ink-soft);
  margin-bottom: 10px;
}
.popup-tag.high {
  background: #fee2e2;
  color: #b91c1c;
}
.popup-content {
  font-size: 14px;
  font-weight: 600;
  line-height: 1.6;
}
.popup-guide {
  font-size: 12px;
  color: var(--ink-mute);
  margin-top: 6px;
  line-height: 1.6;
}
.popup-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}

/* 采集自检 */
.mic-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 2px;
}
.mic-name {
  max-width: 280px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.level-bar {
  width: 120px;
  height: 6px;
  border-radius: 999px;
  background: #e5e9f2;
  overflow: hidden;
  display: inline-block;
}
.level-bar i {
  display: block;
  height: 100%;
  border-radius: 999px;
  background: linear-gradient(90deg, #22c55e, #eab308, #ef4444);
  transition: width 0.25s ease;
}
.mic-warn {
  color: #c2410c;
  font-style: normal;
}
/* 采集异常提示：按应用采集无声音已自动回退系统声音 */
.cap-note {
  color: #b45309;
  font-size: 12px;
}

/* 录制面板工具行：增益 / 应用选择 / 接管 */
.tool-row {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
}
.tool-label {
  flex-shrink: 0;
}
.gain-slider {
  width: 160px;
  accent-color: var(--brand);
}
.gain-val {
  font-variant-numeric: tabular-nums;
  min-width: 44px;
}
.app-select {
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 4px 10px;
  font-size: 12px;
  background: #fff;
  max-width: 240px;
}
.tool-hint {
  font-size: 12px;
  color: var(--ink-mute);
}

/* 继续录制 */
.resume-bar {
  margin-bottom: 12px;
  display: flex;
  gap: 10px;
  flex-wrap: wrap;
}
.flex-spacer {
  flex: 1;
}

/* 实时字幕滚动容器：限高可滚动，回看历史 */
.live-body {
  max-height: 300px;
  overflow-y: auto;
}
.live-latest {
  margin-left: auto;
}

/* 双语文本：整理版 / 原始字幕切换 */
.tidy-switch {
  margin-left: auto;
  display: inline-flex;
  gap: 6px;
}
.panel-head .tidy-switch .btn {
  margin-left: 0;
}
.tidy-hint {
  font-size: 12px;
  color: var(--ink-mute);
}

/* 任务清单 */
.task-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 4px;
  border-bottom: 1px solid var(--line);
}
.task-main {
  flex: 1;
}
.task-content {
  font-size: 13px;
}
.task-guide {
  font-size: 12px;
  color: var(--ink-mute);
  margin-top: 2px;
}
.task-ddl {
  margin-top: 4px;
}
.ddl-pick {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--ink-mute);
  cursor: pointer;
}
.ddl-picker {
  border: 1px solid var(--line);
  border-radius: 6px;
  padding: 2px 6px;
  font-size: 12px;
  background: #fff;
  color: var(--ink-soft);
}

/* 说话人记号 */
.speaker-legend {
  font-size: 12px;
  color: var(--ink-mute);
  background: #f4f6fb;
  border-radius: 8px;
  padding: 8px 12px;
  margin-bottom: 8px;
}
.spk-other {
  color: #b45309;
}

/* 附件分组 */
.mat-group {
  margin-bottom: 14px;
}
.mat-group-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--ink-soft);
  padding: 6px 4px;
  background: #fafbfe;
  border-radius: 6px;
}
.mat-size {
  font-size: 11px;
  color: var(--ink-mute);
  min-width: 56px;
  text-align: right;
}
.danger-text {
  color: var(--danger);
}

/* 附件批量上传分类卡 */
.upload-card {
  width: 460px;
  max-width: calc(100vw - 40px);
}
.up-list {
  margin: 12px 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: 320px;
  overflow-y: auto;
}
.up-row {
  display: flex;
  align-items: center;
  gap: 10px;
}
.up-name {
  flex: 1;
  font-size: 13px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.up-cat {
  border: 1px solid var(--line);
  border-radius: 8px;
  padding: 4px 8px;
  font-size: 12px;
  background: #fff;
  flex-shrink: 0;
}
</style>
