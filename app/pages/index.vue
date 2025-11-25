<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen, type Event, type UnlistenFn } from "@tauri-apps/api/event";

type LogLevel = "info" | "warn" | "error";
type SoftwareKind = "emulator" | "maa";
type SoftwarePhase =
  | "unknown"
  | "idle"
  | "starting"
  | "running"
  | "stopping"
  | "stopped"
  | "error";

interface LogEntry {
  timestamp_ms: number;
  level: LogLevel;
  message: string;
}

interface SoftwareStatus {
  kind: SoftwareKind;
  phase: SoftwarePhase;
  last_message?: string | null;
  last_updated_ms: number;
}

interface CommandOutcome {
  label: string;
  command: string;
  exit_code: number;
  success: boolean;
  stdout: string;
  stderr: string;
}

interface ControlAction {
  key: string;
  label: string;
  command: "start_emulator" | "stop_emulator" | "run_maa_startup";
  intent: "primary" | "neutral" | "danger";
}

const actions: ControlAction[] = [
  {
    key: "start",
    label: "启动模拟器",
    command: "start_emulator",
    intent: "primary",
  },
  {
    key: "stop",
    label: "关闭模拟器",
    command: "stop_emulator",
    intent: "danger",
  },
  {
    key: "maa",
    label: "执行 MAA (startup Official)",
    command: "run_maa_startup",
    intent: "neutral",
  },
];

const isClient = ref(false);
const isTauri = ref(false);
const statuses = ref<SoftwareStatus[]>([]);
const logs = ref<LogEntry[]>([]);
const pendingAction = ref<string | null>(null);
const lastResult = ref<(CommandOutcome & { finishedAt: number }) | null>(null);
const actionError = ref<string | null>(null);
const logPanel = ref<HTMLDivElement | null>(null);

// Tauri-specific: 日志筛选和显示选项
const logFilter = ref<LogLevel | "all">("all");

let unlistenLog: UnlistenFn | null = null;
let unlistenStatus: UnlistenFn | null = null;

const phaseText: Record<SoftwarePhase, string> = {
  unknown: "未知",
  idle: "空闲",
  starting: "启动中",
  running: "运行中",
  stopping: "停止中",
  stopped: "已停止",
  error: "异常",
};

const kindLabel: Record<SoftwareKind, string> = {
  emulator: "模拟器",
  maa: "MAA",
};

const sortedLogs = computed(() => {
  // Tauri-specific: 根据日志级别过滤（UI交互式筛选）
  const filtered =
    logFilter.value === "all"
      ? logs.value
      : logs.value.filter((log) => log.level === logFilter.value);
  return filtered.slice(-200);
});

function formatTimestamp(value: number) {
  const date = new Date(value);
  return `${date.toLocaleDateString()} ${date.toLocaleTimeString()}`;
}

function addLog(entry: LogEntry) {
  logs.value = [...logs.value, entry].slice(-200);
  nextTick(() => {
    if (logPanel.value) {
      logPanel.value.scrollTop = logPanel.value.scrollHeight;
    }
  });
}

// Tauri-specific: 清空日志的交互式控制
function handleClearLogs() {
  if (confirm("确定要清空所有日志吗？此操作无法撤销。")) {
    logs.value = [];
  }
}

// Tauri-specific: 复制错误信息到剪贴板便于调试
async function copyErrorToClipboard() {
  if (!lastResult.value) return;
  const error = [
    `命令: ${lastResult.value.command}`,
    `退出码: ${lastResult.value.exit_code}`,
    `状态: ${lastResult.value.success ? "成功" : "失败"}`,
    "",
    lastResult.value.stdout ? `STDOUT:\n${lastResult.value.stdout}` : "",
    lastResult.value.stderr ? `STDERR:\n${lastResult.value.stderr}` : "",
  ]
    .filter(Boolean)
    .join("\n");

  try {
    await navigator.clipboard.writeText(error);
    alert("错误信息已复制到剪贴板");
  } catch {
    console.error("无法复制到剪贴板");
  }
}

async function refreshStatus() {
  const data = await invoke<SoftwareStatus[]>("fetch_status");
  statuses.value = data;
}

async function refreshLogs() {
  const data = await invoke<LogEntry[]>("fetch_logs");
  logs.value = data;
}

async function runAction(action: ControlAction) {
  if (!isTauri.value || pendingAction.value) {
    return;
  }
  pendingAction.value = action.key;
  actionError.value = null;
  try {
    const result = await invoke<CommandOutcome>(action.command);
    lastResult.value = {
      ...result,
      finishedAt: Date.now(),
    };
    await refreshStatus();
  } catch (error) {
    actionError.value =
      typeof error === "string" ? error : (error as Error).message;
  } finally {
    pendingAction.value = null;
  }
}

onMounted(async () => {
  isClient.value = true;
  isTauri.value =
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  if (!isTauri.value) {
    return;
  }
  await Promise.allSettled([refreshStatus(), refreshLogs()]);
  unlistenLog = await listen<LogEntry>(
    "backend://log",
    (event: Event<LogEntry>) => {
      addLog(event.payload);
    }
  );
  unlistenStatus = await listen<SoftwareStatus>(
    "backend://status",
    (event: Event<SoftwareStatus>) => {
      const current = [...statuses.value];
      const index = current.findIndex(
        (item) => item.kind === event.payload.kind
      );
      if (index >= 0) {
        current[index] = event.payload;
      } else {
        current.push(event.payload);
      }
      statuses.value = current;
    }
  );
});

onBeforeUnmount(() => {
  if (unlistenLog) {
    unlistenLog();
  }
  if (unlistenStatus) {
    unlistenStatus();
  }
});
</script>

<template>
  <main class="page">
    <header class="page__hero">
      <div>
        <p class="eyebrow">Easy MAA 控制台</p>
        <h1>模拟器与 MAA 控制</h1>
        <p class="subheading">
          从 Rust 后端触发模拟器开/关以及
          <code>maa startup Official</code> 指令，并实时查看日志与状态。
        </p>
      </div>
      <div class="status-chip" :class="{ online: isTauri }">
        {{ isTauri ? "已连接 Tauri 后端" : "未检测到 Tauri Runtime" }}
      </div>
    </header>

    <section v-if="!isClient" class="card">
      <p>正在初始化...</p>
    </section>
    <section v-else-if="!isTauri" class="card">
      <p>当前运行环境不是 Tauri 桌面端，请在 Tauri 应用中打开以使用功能。</p>
    </section>

    <section v-else class="grid">
      <div class="card">
        <h2>控制面板</h2>
        <p class="helper">所有命令都会记录在后台日志中。</p>
        <div class="actions">
          <button
            v-for="action in actions"
            :key="action.key"
            :class="[
              'action-btn',
              action.intent,
              { busy: pendingAction === action.key },
            ]"
            :disabled="!!pendingAction"
            @click="runAction(action)"
          >
            <span>{{ action.label }}</span>
            <span
              v-if="pendingAction === action.key"
              class="spinner"
              aria-hidden="true"
            />
          </button>
        </div>
        <p v-if="actionError" class="error-text">{{ actionError }}</p>
        <div v-if="lastResult" class="result">
          <h3>最近结果</h3>
          <p>
            <strong>{{ lastResult.label }}</strong>
            <span :class="['pill', lastResult.success ? 'success' : 'danger']">
              {{ lastResult.success ? "成功" : "失败" }} ({{
                lastResult.exit_code
              }})
            </span>
          </p>
          <p class="command">{{ lastResult.command }}</p>

          <!-- Tauri-specific: 错误详情展示（仅在失败时高亮显示） -->
          <div v-if="!lastResult.success" class="error-details">
            <h4>错误详情</h4>
            <div v-if="lastResult.stderr" class="stderr-box">
              <strong>标准错误输出 (STDERR):</strong>
              <pre>{{ lastResult.stderr }}</pre>
            </div>
            <div v-if="lastResult.stdout" class="stdout-box">
              <strong>标准输出 (STDOUT):</strong>
              <pre>{{ lastResult.stdout }}</pre>
            </div>
            <div
              v-if="!lastResult.stderr && !lastResult.stdout"
              class="no-output"
            >
              <p>无错误输出信息</p>
            </div>
            <button class="copy-btn" @click="copyErrorToClipboard">
              复制错误信息
            </button>
          </div>

          <!-- 成功时显示输出 -->
          <details v-else>
            <summary>展开输出</summary>
            <pre v-if="lastResult.stdout">{{ lastResult.stdout }}</pre>
            <pre v-if="lastResult.stderr" class="stderr">{{
              lastResult.stderr
            }}</pre>
          </details>
          <small>完成时间：{{ formatTimestamp(lastResult.finishedAt) }}</small>
        </div>
      </div>

      <div class="card">
        <h2>软件状态</h2>
        <div v-if="!statuses.length" class="placeholder">尚无状态数据</div>
        <div v-else class="status-list">
          <article
            v-for="status in statuses"
            :key="status.kind"
            class="status-card"
            :data-phase="status.phase"
          >
            <header>
              <strong>{{ kindLabel[status.kind] }}</strong>
              <span class="pill" :class="status.phase">{{
                phaseText[status.phase]
              }}</span>
            </header>
            <p class="message">{{ status.last_message || "暂无消息" }}</p>
            <small
              >更新时间：{{ formatTimestamp(status.last_updated_ms) }}</small
            >
          </article>
        </div>
      </div>
    </section>

    <section v-if="isTauri" class="card logs">
      <header>
        <h2>实时日志</h2>
        <!-- Tauri-specific: 日志筛选和控制工具栏 -->
        <div class="log-controls">
          <div class="filter-group">
            <label>筛选级别：</label>
            <select v-model="logFilter">
              <option value="all">全部</option>
              <option value="info">信息 (Info)</option>
              <option value="warn">警告 (Warn)</option>
              <option value="error">错误 (Error)</option>
            </select>
          </div>
          <button class="ghost" @click="refreshLogs">刷新</button>
          <button class="ghost" @click="handleClearLogs">清空</button>
        </div>
      </header>
      <div ref="logPanel" class="log-panel">
        <template v-if="sortedLogs.length">
          <!-- Tauri-specific: 日志条目根据级别着色（错误高亮显示） -->
          <p
            v-for="(entry, index) in sortedLogs"
            :key="entry.timestamp_ms + index"
            :class="['log-line', entry.level]"
          >
            <span class="log-time">{{
              new Date(entry.timestamp_ms).toLocaleTimeString()
            }}</span>
            <!-- 错误类消息加上特殊标记便于识别 -->
            <span class="log-message">
              <span v-if="entry.level === 'error'" class="error-badge">⚠️</span>
              {{ entry.message }}
            </span>
          </p>
        </template>
        <p v-else class="placeholder">暂无日志</p>
      </div>
    </section>
  </main>
</template>

<style scoped>
:global(body) {
  margin: 0;
  font-family: "Inter", "Noto Sans SC", system-ui, -apple-system,
    BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: #0f172a;
  color: #e2e8f0;
}

.page {
  min-height: 100vh;
  padding: 2rem;
  max-width: 1200px;
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  gap: 1.5rem;
}

.page__hero {
  display: flex;
  justify-content: space-between;
  gap: 1rem;
  align-items: center;
}

.eyebrow {
  text-transform: uppercase;
  letter-spacing: 0.2em;
  font-size: 0.75rem;
  color: #94a3b8;
  margin: 0 0 0.5rem 0;
}

.subheading {
  color: #94a3b8;
  margin-top: 0.5rem;
}

.status-chip {
  padding: 0.5rem 1rem;
  border-radius: 999px;
  background: #334155;
  font-size: 0.85rem;
}

.status-chip.online {
  background: #0f766e;
}

.grid {
  display: grid;
  gap: 1.5rem;
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
}

.card {
  background: #1e293b;
  border-radius: 1rem;
  padding: 1.5rem;
  box-shadow: 0 20px 40px rgba(15, 23, 42, 0.4);
}

.card h2 {
  margin-top: 0;
  margin-bottom: 0.5rem;
}

.helper {
  color: #94a3b8;
  margin-bottom: 1rem;
}

.actions {
  display: flex;
  flex-wrap: wrap;
  gap: 0.75rem;
}

.action-btn {
  display: inline-flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.75rem 1.25rem;
  border-radius: 0.75rem;
  border: none;
  cursor: pointer;
  font-weight: 600;
  color: #0f172a;
  transition: opacity 0.2s ease;
}

.action-btn.primary {
  background: #38bdf8;
}
.action-btn.neutral {
  background: #cbd5f5;
}
.action-btn.danger {
  background: #f87171;
}

.action-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.spinner {
  width: 16px;
  height: 16px;
  border-radius: 50%;
  border: 2px solid rgba(15, 23, 42, 0.3);
  border-top-color: rgba(15, 23, 42, 0.9);
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.error-text {
  color: #fda4af;
  margin-top: 0.75rem;
}

.result {
  margin-top: 1.5rem;
  border-top: 1px solid rgba(148, 163, 184, 0.3);
  padding-top: 1rem;
}

/* Tauri-specific: 错误详情展示面板（失败时显示） */
.error-details {
  margin-top: 1rem;
  padding: 1rem;
  background: rgba(248, 113, 113, 0.1);
  border-left: 4px solid #f87171;
  border-radius: 0.5rem;
}

.error-details h4 {
  margin-top: 0;
  color: #f87171;
  margin-bottom: 0.75rem;
}

.stderr-box,
.stdout-box {
  margin-bottom: 0.75rem;
}

.stderr-box strong,
.stdout-box strong {
  display: block;
  margin-bottom: 0.5rem;
  color: #fca5a5;
}

.no-output {
  color: #cbd5f5;
  font-style: italic;
  padding: 0.5rem 0;
}

.copy-btn {
  margin-top: 0.75rem;
  padding: 0.4rem 0.75rem;
  background: #f87171;
  color: #0f172a;
  border: none;
  border-radius: 0.4rem;
  cursor: pointer;
  font-size: 0.85rem;
  font-weight: 600;
  transition: background 0.2s ease;
}

.copy-btn:hover {
  background: #fb7185;
}

.pill {
  padding: 0.2rem 0.8rem;
  border-radius: 999px;
  font-size: 0.8rem;
  color: #0f172a;
  background: #94a3b8;
}

.pill.success {
  background: #4ade80;
}
.pill.danger {
  background: #f87171;
}

.result pre {
  background: #0f172a;
  padding: 0.75rem;
  border-radius: 0.5rem;
  overflow-x: auto;
}

.result pre.stderr {
  border: 1px solid rgba(248, 113, 113, 0.4);
}

.status-list {
  display: flex;
  flex-direction: column;
  gap: 1rem;
}

.status-card {
  padding: 1rem;
  border-radius: 0.75rem;
  background: #0f172a;
  border: 1px solid transparent;
}

.status-card[data-phase="running"] {
  border-color: rgba(34, 197, 94, 0.4);
}
.status-card[data-phase="error"] {
  border-color: rgba(248, 113, 113, 0.6);
}

.status-card header {
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.status-card .message {
  margin: 0.75rem 0;
  color: #e2e8f0;
}

.logs header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-wrap: wrap;
  gap: 0.75rem;
}

/* Tauri-specific: 日志筛选和控制工具栏 */
.log-controls {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  flex-wrap: wrap;
}

.filter-group {
  display: flex;
  align-items: center;
  gap: 0.5rem;
}

.filter-group label {
  font-size: 0.85rem;
  color: #94a3b8;
}

.filter-group select {
  background: #0f172a;
  border: 1px solid #475569;
  color: #e2e8f0;
  padding: 0.35rem 0.6rem;
  border-radius: 0.4rem;
  font-size: 0.85rem;
  cursor: pointer;
}

.filter-group select:hover,
.filter-group select:focus {
  border-color: #38bdf8;
  outline: none;
}

.ghost {
  background: transparent;
  border: 1px solid #38bdf8;
  color: #38bdf8;
  padding: 0.4rem 0.8rem;
  border-radius: 0.5rem;
  cursor: pointer;
}

.log-panel {
  margin-top: 1rem;
  max-height: 360px;
  overflow-y: auto;
  background: #0b1120;
  border-radius: 0.75rem;
  padding: 1rem;
}

.log-line {
  display: flex;
  gap: 0.5rem;
  font-family: "JetBrains Mono", "Fira Code", monospace;
  font-size: 0.85rem;
  margin: 0.2rem 0;
}

.log-line.info .log-message {
  color: #e2e8f0;
}
.log-line.warn .log-message {
  color: #fbbf24;
}
.log-line.error .log-message {
  color: #f87171;
  font-weight: 600;
}

/* Tauri-specific: 错误消息标记 */
.error-badge {
  margin-right: 0.25rem;
}

.log-time {
  color: #94a3b8;
}

.placeholder {
  color: #94a3b8;
  text-align: center;
  padding: 1rem 0;
}

@media (max-width: 768px) {
  .page {
    padding: 1.5rem;
  }
  .page__hero {
    flex-direction: column;
    align-items: flex-start;
  }
  .actions {
    flex-direction: column;
  }
  .action-btn {
    width: 100%;
    justify-content: center;
  }
}
</style>
