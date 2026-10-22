<template>
  <div class="page narrow" v-if="appStore.settings">
    <div class="page-head">
      <h1>设置</h1>
    </div>

    <!-- 1. 学习与课表 -->
    <div class="card sec">
      <div class="sec-toggle" @click="toggle('study')">
        <span class="caret">{{ open.study ? "▾" : "▸" }}</span>
        <h3>学习与课表</h3>
        <span class="sec-summary">学期 · 语言 · 课程表时间</span>
      </div>
      <div v-show="open.study" class="sec-body">
        <div class="field">
          <label>第一周周一（用于周次计算与课程表）</label>
          <input v-model="form.semesterStart" type="date" class="input max-260" />
          <div class="hint">当前日期为第 {{ currentWeek }} 周</div>
        </div>
        <div class="row-2">
          <div class="field">
            <label>课堂语言</label>
            <select v-model="form.speakLanguage" class="select">
              <option value="en">英语</option>
              <option value="zh">中文</option>
            </select>
          </div>
          <div class="field">
            <label>日常语言</label>
            <select v-model="form.dailyLanguage" class="select">
              <option value="zh">中文</option>
              <option value="en">英语</option>
            </select>
          </div>
        </div>
        <div class="field">
          <label>课程表时间范围（整点，每小时一小节）</label>
          <div class="time-range">
            <select v-model.number="form.scheduleStartHour" class="select max-120">
              <option v-for="h in startHourOptions" :key="h" :value="h">{{ h }}:00</option>
            </select>
            <span class="dash">至</span>
            <select v-model.number="form.scheduleEndHour" class="select max-120">
              <option v-for="h in endHourOptions" :key="h" :value="h">{{ h === 24 ? "24:00" : `${h}:00` }}</option>
            </select>
          </div>
          <div v-if="form.scheduleStartHour >= form.scheduleEndHour" class="hint err">
            开始时间需早于结束时间
          </div>
          <div v-else class="hint">课程表共显示 {{ form.scheduleEndHour - form.scheduleStartHour }} 个小节</div>
        </div>
      </div>
    </div>

    <!-- 2. 录制与检测 -->
    <div class="card sec">
      <div class="sec-toggle" @click="toggle('record')">
        <span class="caret">{{ open.record ? "▾" : "▸" }}</span>
        <h3>录制与检测</h3>
        <span class="sec-summary">在位检测 · 摄像头 · 麦克风</span>
      </div>
      <div v-show="open.record" class="sec-body">
        <div class="field">
          <label>在位检测周期（秒，0 表示关闭）</label>
          <input
            v-model.number="form.presenceIntervalSec"
            type="number"
            min="0"
            max="300"
            class="input max-260"
          />
          <div class="hint">每 {{ form.presenceIntervalSec }} 秒检测一帧，连续 2 帧判定离开/回位</div>
        </div>
        <div class="field">
          <label>摄像头人脸检测</label>
          <label class="check-row">
            <input v-model="form.presenceCameraEnabled" type="checkbox" />
            <span>录制时用摄像头判定是否离开座位</span>
          </label>
          <div class="hint">
            开启后每 {{ form.presenceIntervalSec }} 秒抓取一帧画面做人脸检测，连续 2 帧无人脸判定离开。
            开启期间摄像头指示灯会常亮；摄像头不可用时状态标记为「不可用」并提示原因。
          </div>
          <div class="probe-row">
            <button class="btn ghost sm" :disabled="probeBusy" @click="probeCamera">
              {{ probeBusy ? "检测中…" : "测试摄像头" }}
            </button>
            <span v-if="probeResult" class="hint" :class="{ err: !probeResult.ok }">
              {{ probeResult.text }}
            </span>
          </div>
        </div>
        <div class="field">
          <label>麦克风测试</label>
          <MicTestPanel />
        </div>
      </div>
    </div>

    <!-- 3. 模型、密钥与预算 -->
    <div class="card sec">
      <div class="sec-toggle" @click="toggle('model')">
        <span class="caret">{{ open.model ? "▾" : "▸" }}</span>
        <h3>模型、密钥与预算</h3>
        <span class="sec-summary">API Key · 邮件提醒 · 月度预算</span>
      </div>
      <div v-show="open.model" class="sec-body">
        <div class="cfg">
          <div class="cfg-head">
            <span>硅基流动 API Key</span>
            <span class="state" :class="secret.siliconflowKeySet ? 'ok' : 'no'">
              {{ secret.siliconflowKeySet ? `已配置 ${secret.siliconflowKeyMasked}` : "未配置" }}
            </span>
          </div>
          <div class="cfg-body">
            <input v-model="sfKey" type="password" class="input" placeholder="sk-..." />
            <div class="btn-row">
              <button class="btn sm" :disabled="cfgBusy" @click="saveSf">保存</button>
              <button
                class="btn ghost sm"
                :disabled="cfgBusy || !secret.siliconflowKeySet"
                @click="testSf"
              >
                测试连接
              </button>
              <button
                class="btn ghost sm"
                :disabled="cfgBusy || !secret.siliconflowKeySet"
                @click="clearSf"
              >
                清除
              </button>
            </div>
          </div>
          <div class="hint">用于翻译、行动项、笔记、经济模式转写与同桌对话；未配置时课堂归档自动降级为本地模拟数据。</div>
        </div>

        <div class="cfg">
          <div class="cfg-head">
            <span>火山引擎（豆包同传）API Key</span>
            <span class="state" :class="secret.volcKeySet ? 'ok' : 'no'">
              {{ secret.volcKeySet ? `已配置 ${secret.volcKeyMasked}` : "未配置" }}
            </span>
          </div>
          <div class="cfg-body">
            <input v-model="volcKey" type="password" class="input" placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx" />
            <div class="btn-row">
              <button class="btn sm" :disabled="cfgBusy" @click="saveVolc">保存</button>
              <button
                class="btn ghost sm"
                :disabled="cfgBusy || !secret.volcKeySet"
                @click="testVolc"
              >
                测试连接
              </button>
              <button
                class="btn ghost sm"
                :disabled="cfgBusy || !secret.volcKeySet"
                @click="clearVolc"
              >
                清除
              </button>
            </div>
          </div>
          <div class="hint">
            配置后录制时实时字幕走 AST 2.0 同声传译（延迟约 2 秒，边说边出字）；未配置时默认使用免费的经济模式转写。
          </div>
        </div>

        <div class="cfg">
          <div class="cfg-head">
            <span>QQ 邮箱提醒（SMTP）</span>
            <span class="state" :class="secret.smtpUser && secret.smtpAuthSet ? 'ok' : 'no'">
              {{ secret.smtpUser && secret.smtpAuthSet ? "已配置" : "未配置" }}
            </span>
          </div>
          <div class="cfg-body">
            <div class="row-3">
              <input v-model="smtp.user" class="input" placeholder="QQ 邮箱地址" />
              <input v-model="smtp.to" class="input" placeholder="接收邮箱（留空=发给自己）" />
              <input v-model="smtp.auth" type="password" class="input" placeholder="SMTP 授权码" />
            </div>
            <div class="btn-row">
              <button class="btn sm" :disabled="cfgBusy" @click="saveMail">保存</button>
              <button
                class="btn ghost sm"
                :disabled="cfgBusy || !secret.smtpUser || !secret.smtpAuthSet"
                @click="testMail"
              >
                发送测试邮件
              </button>
              <button class="btn ghost sm" :disabled="cfgBusy" @click="clearMail">清除</button>
            </div>
          </div>
          <div class="hint">不在位且存在待处理行动项时，桌面弹窗与邮件双通道同时提醒。</div>
        </div>

        <div class="field" style="margin-top: 14px">
          <label>月度花费预算（元）</label>
          <input v-model.number="form.monthlyBudget" type="number" min="1" class="input max-260" />
          <div class="hint">达到预算 80% / 100% 时发送桌面通知提醒</div>
        </div>

        <div class="hint">密钥通过 Windows DPAPI 加密保存到本机 secrets.json，不写入数据库、不上传。</div>
        <transition name="fade">
          <div v-if="cfgMsg" class="cfg-msg">{{ cfgMsg }}</div>
        </transition>
      </div>
    </div>

    <!-- 4. 小轩人设与提示词 -->
    <div class="card sec">
      <div class="sec-toggle" @click="toggle('persona')">
        <span class="caret">{{ open.persona ? "▾" : "▸" }}</span>
        <h3>小轩人设与提示词</h3>
        <span class="sec-summary">聊天风格 · 各任务提示词模板</span>
      </div>
      <div v-show="open.persona" class="sec-body">
        <div class="hint" style="margin-bottom: 12px">
          人设固定写入同桌对话的系统提示词，对所有页面的聊天生效；如需更深度定制，可在本板块下方编辑「同桌对话」提示词模板。
        </div>
        <div v-for="d in personaDims" :key="d.key" class="persona-row">
          <span class="persona-label">{{ d.label }}</span>
          <div class="seg">
            <button
              v-for="o in d.options"
              :key="o.v"
              class="seg-btn"
              :class="{ on: personaForm[d.key] === o.v }"
              @click="personaForm[d.key] = o.v"
            >
              {{ o.t }}
            </button>
          </div>
        </div>
        <div class="field" style="margin-top: 12px">
          <label>自定义补充（追加到人设之后，可留空）</label>
          <textarea
            v-model="personaForm.extra"
            class="textarea"
            rows="3"
            placeholder="例如：称呼我为宇轩；解释概念时喜欢用生活化的例子…"
          ></textarea>
        </div>
        <div class="btn-row">
          <button class="btn ghost sm" @click="resetPersona">恢复默认人设</button>
        </div>
        <div class="hint" style="margin: 8px 0 14px">人设调整后点击右下角「保存设置」生效。</div>

        <label class="sub-title">系统提示词模板</label>
        <div class="hint" style="margin-bottom: 12px">
          自定义各任务的大模型提示词模板。支持占位符：
          <code>{speak_language}</code> 课堂语言、<code>{daily_language}</code> 日常语言、
          <code>{material_instruction}</code> 附件指引（笔记生成）、<code>{title}</code>
          课堂标题、<code>{context}</code> 课堂记录（同桌对话）。留空内容不可保存；与默认一致时自动视为未覆盖。
        </div>
        <div v-if="promptsLoading" class="hint">加载中…</div>
        <template v-else>
          <div v-for="p in promptForms" :key="p.task" class="prompt-item">
            <div class="prompt-title">
              <span>{{ p.label }}</span>
              <span v-if="!p.isDefault" class="tag-custom">已自定义</span>
            </div>
            <textarea v-model="p.template" class="textarea" rows="4" spellcheck="false"></textarea>
            <div class="btn-row">
              <button class="btn sm" :disabled="promptBusy" @click="savePrompt(p)">保存</button>
              <button
                class="btn ghost sm"
                :disabled="promptBusy || p.isDefault"
                @click="resetPrompt(p)"
              >
                恢复默认
              </button>
            </div>
          </div>
          <div class="btn-row" style="margin-top: 4px">
            <button class="btn ghost sm" :disabled="promptBusy" @click="resetAllPrompts">
              全部恢复默认
            </button>
          </div>
        </template>
        <transition name="fade">
          <div v-if="promptMsg" class="cfg-msg">{{ promptMsg }}</div>
        </transition>
      </div>
    </div>

    <!-- 5. 数据、日志与回收站 -->
    <div class="card sec">
      <div class="sec-toggle" @click="toggle('data')">
        <span class="caret">{{ open.data ? "▾" : "▸" }}</span>
        <h3>数据、日志与回收站</h3>
        <span class="sec-summary">调用日志 · 已删除课堂</span>
      </div>
      <div v-show="open.data" class="sec-body">
        <div class="data-link-row">
          <div>
            <b>大模型调用日志</b>
            <div class="hint">记录每次大模型调用的输入、输出、token 用量与错误信息，最多保留最近 1000 条。</div>
          </div>
          <button class="btn ghost sm" @click="router.push('/llm-logs')">查看日志</button>
        </div>

        <div class="trash-head">
          <b>课堂回收站</b>
          <button class="btn ghost sm" :disabled="trashLoading" @click="loadTrash">
            {{ trashLoading ? "…" : "↻ 刷新" }}
          </button>
        </div>
        <div class="hint" style="margin-bottom: 12px">
          删除的课堂在此保留，可恢复或彻底删除。彻底删除会同时清除录音、字幕、行动项、用量与调用日志，不可恢复。
        </div>
        <div v-if="trashItems.length === 0" class="hint">回收站为空。</div>
        <div v-for="s in trashItems" :key="s.id" class="prompt-item trash-row">
          <div class="trash-info">
            <b>{{ s.title }}</b>
            <span class="hint">
              {{ s.startAt.slice(0, 16).replace("T", " ") }} · 删除于 {{ (s.deletedAt ?? "").slice(0, 16).replace("T", " ") }}
            </span>
          </div>
          <div class="btn-row">
            <button class="btn ghost sm" :disabled="trashBusy" @click="restoreOne(s.id)">恢复</button>
            <button class="btn danger sm" :disabled="trashBusy" @click="purgeOne(s.id)">彻底删除</button>
          </div>
        </div>
      </div>
    </div>

    <div class="save-row">
      <transition name="fade">
        <span v-if="savedTip" class="saved-tip">✓ 已保存</span>
      </transition>
      <button class="btn" @click="save">保存设置</button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { useAppStore } from "../stores/app";
import { api } from "../api/ipc";
import { weekNumberOf } from "../utils/date";
import MicTestPanel from "../components/MicTestPanel.vue";
import type { ClassSession, PromptTemplate, SecretConfig } from "../types";

const router = useRouter();

const appStore = useAppStore();

/* 折叠板块：默认全部收起，点击标题行展开/收起 */
const open = reactive<Record<string, boolean>>({
  study: false,
  record: false,
  model: false,
  persona: false,
  data: false,
});
function toggle(key: string) {
  open[key] = !open[key];
}

const form = reactive({
  semesterStart: appStore.settings!.semesterStart,
  monthlyBudget: appStore.settings!.monthlyBudget,
  speakLanguage: appStore.settings!.speakLanguage,
  dailyLanguage: appStore.settings!.dailyLanguage,
  presenceIntervalSec: appStore.settings!.presenceIntervalSec,
  presenceCameraEnabled: appStore.settings!.presenceCameraEnabled,
  scheduleStartHour: appStore.settings!.scheduleStartHour,
  scheduleEndHour: appStore.settings!.scheduleEndHour,
});

/** 开始小时 0–23；结束小时 1–24（24:00），必须比开始晚 */
const startHourOptions = computed(() =>
  Array.from({ length: 24 }, (_, i) => i).filter((h) => h < form.scheduleEndHour)
);
const endHourOptions = computed(() =>
  Array.from({ length: 24 }, (_, i) => i + 1).filter((h) => h > form.scheduleStartHour)
);

const currentWeek = computed(() => weekNumberOf(new Date(), form.semesterStart));
const savedTip = ref(false);

/* ------------------------------ 密钥配置 ------------------------------ */

const secret = ref<SecretConfig>({
  siliconflowKeySet: false,
  siliconflowKeyMasked: "",
  smtpUser: "",
  smtpTo: "",
  smtpAuthSet: false,
  volcKeySet: false,
  volcKeyMasked: "",
});
const sfKey = ref("");
const volcKey = ref("");
const smtp = reactive({ user: "", to: "", auth: "" });
const cfgMsg = ref("");
const cfgBusy = ref(false);

/* ------------------------------ 课堂回收站 ------------------------------ */

const trashItems = ref<ClassSession[]>([]);
const trashLoading = ref(false);
const trashBusy = ref(false);

async function loadTrash() {
  trashLoading.value = true;
  try {
    trashItems.value = await api.listDeletedSessions();
  } catch (e) {
    flash(`读取回收站失败：${e}`);
  } finally {
    trashLoading.value = false;
  }
}

async function restoreOne(id: number) {
  trashBusy.value = true;
  try {
    await api.restoreSession(id);
    await loadTrash();
    flash("已恢复课堂");
  } catch (e) {
    flash(`恢复失败：${e}`);
  } finally {
    trashBusy.value = false;
  }
}

async function purgeOne(id: number) {
  if (!confirm("彻底删除后录音与全部记录不可恢复，确定继续？")) return;
  trashBusy.value = true;
  try {
    await api.purgeSession(id);
    await loadTrash();
    flash("已彻底删除");
  } catch (e) {
    flash(`删除失败：${e}`);
  } finally {
    trashBusy.value = false;
  }
}

onMounted(async () => {
  try {
    secret.value = await api.getSecretConfig();
    smtp.user = secret.value.smtpUser;
    smtp.to = secret.value.smtpTo;
  } catch (e) {
    flash(`读取配置失败：${e}`);
  }
  void loadTrash();
});

function flash(text: string) {
  cfgMsg.value = text;
  setTimeout(() => {
    if (cfgMsg.value === text) cfgMsg.value = "";
  }, 3200);
}

async function saveSf() {
  const key = sfKey.value.trim();
  if (!key) return flash("请输入 API Key");
  cfgBusy.value = true;
  try {
    secret.value = await api.saveSecretConfig({ siliconflowKey: key });
    sfKey.value = "";
    flash("API Key 已保存");
  } catch (e) {
    flash(`保存失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function testSf() {
  cfgBusy.value = true;
  try {
    const r = await api.testSiliconflow();
    flash(`连接正常：${r}`);
  } catch (e) {
    flash(`测试失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function clearSf() {
  cfgBusy.value = true;
  try {
    secret.value = await api.saveSecretConfig({ clearSiliconflow: true });
    flash("已清除 API Key");
  } catch (e) {
    flash(`清除失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function saveVolc() {
  const key = volcKey.value.trim();
  if (!key) return flash("请输入火山引擎 API Key");
  cfgBusy.value = true;
  try {
    secret.value = await api.saveSecretConfig({ volcKey: key });
    volcKey.value = "";
    flash("火山引擎 API Key 已保存");
  } catch (e) {
    flash(`保存失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function testVolc() {
  cfgBusy.value = true;
  try {
    const r = await api.testVolc();
    flash(r);
  } catch (e) {
    flash(`测试失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function clearVolc() {
  cfgBusy.value = true;
  try {
    secret.value = await api.saveSecretConfig({ clearVolc: true });
    flash("已清除火山引擎 API Key");
  } catch (e) {
    flash(`清除失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function saveMail() {
  cfgBusy.value = true;
  try {
    secret.value = await api.saveSecretConfig({
      smtpUser: smtp.user.trim(),
      smtpTo: smtp.to.trim(),
      smtpAuthCode: smtp.auth.trim() || undefined,
    });
    smtp.auth = "";
    flash("邮箱配置已保存");
  } catch (e) {
    flash(`保存失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function testMail() {
  cfgBusy.value = true;
  try {
    await api.testMail();
    flash("测试邮件已发送，请查收");
  } catch (e) {
    flash(`发送失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

async function clearMail() {
  cfgBusy.value = true;
  try {
    secret.value = await api.saveSecretConfig({ clearSmtp: true });
    smtp.user = "";
    smtp.to = "";
    smtp.auth = "";
    flash("已清除邮箱配置");
  } catch (e) {
    flash(`清除失败：${e}`);
  } finally {
    cfgBusy.value = false;
  }
}

/* ------------------------------ 同桌人设矩阵 ------------------------------ */

/** 三维度三档 + 自定义补充；detail/warmth/humor 用宽松 string 以便模板循环赋值 */
const personaForm = reactive<{ detail: string; warmth: string; humor: string; extra: string }>({
  detail: "mid",
  warmth: "mid",
  humor: "mid",
  extra: "",
});

// 从已保存设置读取（JSON 解析失败按默认中档）
try {
  const p = JSON.parse(appStore.settings!.persona ?? "{}") as Record<string, string>;
  personaForm.detail = p.detail ?? "mid";
  personaForm.warmth = p.warmth ?? "mid";
  personaForm.humor = p.humor ?? "mid";
  personaForm.extra = p.extra ?? "";
} catch {
  /* 保持默认中档 */
}

const personaDims = [
  {
    key: "detail" as const,
    label: "回答详略",
    options: [
      { v: "low", t: "简洁" },
      { v: "mid", t: "均衡" },
      { v: "high", t: "细致" },
    ],
  },
  {
    key: "warmth" as const,
    label: "语气温度",
    options: [
      { v: "cold", t: "高冷" },
      { v: "mid", t: "随和" },
      { v: "warm", t: "热情" },
    ],
  },
  {
    key: "humor" as const,
    label: "聊天风格",
    options: [
      { v: "serious", t: "严肃" },
      { v: "mid", t: "适中" },
      { v: "fun", t: "幽默" },
    ],
  },
];

function resetPersona() {
  personaForm.detail = "mid";
  personaForm.warmth = "mid";
  personaForm.humor = "mid";
  personaForm.extra = "";
}

async function save() {
  if (form.scheduleStartHour >= form.scheduleEndHour) return;
  // 人设矩阵：全部中档且无自定义补充时存 null（视为默认），否则存 JSON
  const hasExtra = personaForm.extra.trim().length > 0;
  const personaDefault =
    personaForm.detail === "mid" && personaForm.warmth === "mid" && personaForm.humor === "mid" && !hasExtra;
  await appStore.save({
    ...form,
    persona: personaDefault
      ? null
      : JSON.stringify({
          detail: personaForm.detail,
          warmth: personaForm.warmth,
          humor: personaForm.humor,
          extra: hasExtra ? personaForm.extra.trim() : undefined,
        }),
  });
  savedTip.value = true;
  setTimeout(() => (savedTip.value = false), 1800);
}

/* ------------------------------ 摄像头探测 ------------------------------ */

const probeBusy = ref(false);
const probeResult = ref<{ ok: boolean; text: string } | null>(null);

/** 单次探测摄像头人脸检测是否可用；结果仅用于提示，不影响设置保存。 */
async function probeCamera() {
  probeBusy.value = true;
  probeResult.value = null;
  try {
    const r = await api.probeCamera();
    probeResult.value = {
      ok: r.ok && r.faces === true,
      text:
        r.faces === true
          ? "摄像头正常，画面中已检测到人脸"
          : (r.detail ?? "摄像头不可用"),
    };
  } catch (e) {
    probeResult.value = { ok: false, text: `检测失败：${e}` };
  } finally {
    probeBusy.value = false;
  }
}

/* ------------------------------ 提示词模板 ------------------------------ */

interface PromptForm {
  task: string;
  label: string;
  template: string;
  isDefault: boolean;
}

const promptForms = ref<PromptForm[]>([]);
const promptsLoading = ref(true);
const promptBusy = ref(false);
const promptMsg = ref("");

function flashPrompt(text: string) {
  promptMsg.value = text;
  setTimeout(() => {
    if (promptMsg.value === text) promptMsg.value = "";
  }, 3200);
}

function toForms(list: PromptTemplate[]): PromptForm[] {
  return list.map((p) => ({ task: p.task, label: p.label, template: p.template, isDefault: p.isDefault }));
}

onMounted(async () => {
  try {
    promptForms.value = toForms(await api.getPromptTemplates());
  } catch (e) {
    flashPrompt(`读取提示词模板失败：${e}`);
  } finally {
    promptsLoading.value = false;
  }
});

async function savePrompt(p: PromptForm) {
  if (!p.template.trim()) return flashPrompt(`「${p.label}」模板内容不能为空`);
  promptBusy.value = true;
  try {
    await api.savePromptTemplate(p.task, p.template);
    p.isDefault = false;
    flashPrompt(`「${p.label}」已保存`);
  } catch (e) {
    flashPrompt(`保存失败：${e}`);
  } finally {
    promptBusy.value = false;
  }
}

async function resetPrompt(p: PromptForm) {
  promptBusy.value = true;
  try {
    const list = await api.getPromptTemplates();
    const def = list.find((x) => x.task === p.task);
    if (!def) throw new Error("模板不存在");
    // 保存默认模板内容即触发后端删除覆盖记录
    await api.savePromptTemplate(p.task, def.template);
    p.template = def.template;
    p.isDefault = true;
    flashPrompt(`「${p.label}」已恢复默认`);
  } catch (e) {
    flashPrompt(`恢复失败：${e}`);
  } finally {
    promptBusy.value = false;
  }
}

async function resetAllPrompts() {
  promptBusy.value = true;
  try {
    await api.resetPromptTemplates();
    promptForms.value = toForms(await api.getPromptTemplates());
    flashPrompt("全部模板已恢复默认");
  } catch (e) {
    flashPrompt(`恢复失败：${e}`);
  } finally {
    promptBusy.value = false;
  }
}
</script>

<style scoped>
.narrow {
  /* 不限最大宽度：随外层窗口拉伸；仅保底最小宽度，过窄时由外层出横向滚动条 */
  min-width: 640px;
}
.sec {
  padding: 0;
  margin-bottom: 12px;
  overflow: hidden;
}
/* 折叠标题行：整块可点击，右侧小三角提示展开态 */
.sec-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 16px 22px;
  cursor: pointer;
  user-select: none;
}
.sec-toggle:hover {
  background: #fafbfe;
}
.sec-toggle h3 {
  margin: 0;
  font-size: 14px;
}
.caret {
  width: 14px;
  color: var(--ink-mute);
  font-size: 12px;
  text-align: center;
}
.sec-summary {
  margin-left: auto;
  font-size: 12px;
  color: var(--ink-mute);
}
.sec-body {
  padding: 4px 22px 18px;
  border-top: 1px solid var(--line);
}
.sub-title {
  display: block;
  font-size: 13px;
  font-weight: 700;
  margin: 18px 0 10px;
}
.row-2 {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 0 16px;
}
.row-3 {
  display: grid;
  grid-template-columns: 1fr 1fr 1fr;
  gap: 0 12px;
}
.max-260 {
  max-width: 260px;
}
.max-120 {
  max-width: 120px;
}
.time-range {
  display: flex;
  align-items: center;
  gap: 10px;
}
.dash {
  color: var(--ink-mute);
  font-size: 13px;
}
.hint.err {
  color: #c2410c;
}
.cfg {
  padding: 12px 0;
  border-bottom: 1px dashed var(--line);
}
.cfg-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  font-size: 13px;
  font-weight: 600;
}
.state {
  font-size: 12px;
  font-weight: 600;
  color: var(--ink-soft);
}
.state.ok {
  color: #0f9d58;
}
.state.no {
  color: #c2410c;
}
.cfg-body {
  margin-top: 10px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}
.btn-row {
  display: flex;
  gap: 8px;
}
.check-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  margin-bottom: 6px;
}
.persona-row {
  display: flex;
  align-items: center;
  gap: 14px;
  padding: 8px 0;
}
.persona-label {
  width: 76px;
  font-size: 13px;
  color: var(--ink-soft);
  flex-shrink: 0;
}
.seg {
  display: inline-flex;
  border: 1px solid var(--line);
  border-radius: 10px;
  overflow: hidden;
}
.seg-btn {
  border: none;
  background: #fff;
  padding: 6px 16px;
  font-size: 12px;
  color: var(--ink-soft);
  cursor: pointer;
  border-right: 1px solid var(--line);
}
.seg-btn:last-child {
  border-right: none;
}
.seg-btn.on {
  background: var(--brand);
  color: #fff;
  font-weight: 600;
}
.probe-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 8px;
  flex-wrap: wrap;
}
.cfg-msg {
  margin-top: 10px;
  font-size: 13px;
  color: var(--accent);
  font-weight: 600;
}
.save-row {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 12px;
}
.saved-tip {
  font-size: 13px;
  color: var(--accent);
  font-weight: 600;
}
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.3s;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
.prompt-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
}
.prompt-head h3 {
  margin: 0;
  font-size: 14px;
}
.data-link-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 0 14px;
  border-bottom: 1px dashed var(--line);
  margin-bottom: 14px;
}
.trash-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 8px;
  font-size: 13px;
}
.prompt-item {
  padding: 12px 0;
  border-bottom: 1px dashed var(--line);
}
.prompt-item:last-of-type {
  border-bottom: none;
}
.trash-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.trash-info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  font-size: 13px;
}
.trash-info .hint {
  font-size: 12px;
}
.prompt-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 13px;
  font-weight: 600;
  margin-bottom: 8px;
}
.tag-custom {
  font-size: 11px;
  font-weight: 600;
  color: var(--accent);
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  padding: 2px 8px;
  border-radius: 999px;
}
.textarea {
  width: 100%;
  box-sizing: border-box;
  font-family: inherit;
  font-size: 13px;
  line-height: 1.6;
  padding: 10px 12px;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: var(--bg);
  color: var(--ink);
  resize: vertical;
  margin-bottom: 8px;
}
.textarea:focus {
  outline: none;
  border-color: var(--accent);
}
code {
  font-size: 12px;
  background: color-mix(in srgb, var(--ink) 8%, transparent);
  padding: 1px 5px;
  border-radius: 4px;
}
</style>
