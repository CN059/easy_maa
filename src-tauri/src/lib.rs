//! Easy MAA 后端核心模块
//! 
//! 本模块实现了 Tauri 应用的核心业务逻辑，包括：
//! - 命令执行和管理（emulator 启动/停止、MAA 任务执行）
//! - 日志和状态实时推送到前端
//! - Sudo 权限分离处理
//! - Server 酱推送支持
//! 
//! ## 事件系统
//! 
//! 后端通过以下事件通道与前端通信：
//! - `backend://log` - 日志事件（实时推送新增日志）
//! - `backend://status` - 状态事件（软件状态变化更新）
//! 
//! ## 指令处理流程
//! 
//! 1. 前端调用 Tauri 指令（如 `start_emulator`）
//! 2. 后端更新状态为 "Starting"
//! 3. 后端在阻塞线程中执行命令（使用 `sudo -n` 如果需要）
//! 4. 捕获 STDOUT/STDERR 和退出码
//! 5. 发送状态更新和日志到前端
//! 6. 返回执行结果给前端

mod config;
mod notifier;

use config::{AppConfig, CommandConfig};
use notifier::send_server_chan;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// 前端监听的日志事件通道名称
const LOG_EVENT: &str = "backend://log";

/// 前端监听的状态更新事件通道名称
const STATUS_EVENT: &str = "backend://status";

/// 内存中保留的最大日志条数（超过此数会自动删除最旧的）
const MAX_MEMORY_LOGS: usize = 200;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  // 加载配置文件（支持 TOML 格式、环境变量和默认值）
  let config = AppConfig::load();

  tauri::Builder::default()
    // 将应用状态注入到所有命令处理器中
    .manage(AppState::new(config))
    .setup(|app| {
      // Debug 构建时启用日志插件
      if cfg!(debug_assertions) {
        app
          .handle()
          .plugin(
            tauri_plugin_log::Builder::default()
              .level(log::LevelFilter::Info)
              .build(),
          )?;
      }
      Ok(())
    })
    // 注册所有 Tauri 指令处理器
    .invoke_handler(tauri::generate_handler![
      start_emulator,
      stop_emulator,
      run_maa_startup,
      fetch_status,
      fetch_logs
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

/// 日志级别
///
/// 用于分类不同的日志消息，便于前端过滤和展示
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
enum LogLevel {
  /// 信息级别日志
  Info,
  /// 警告级别日志
  Warn,
  /// 错误级别日志
  Error,
}

/// 单条日志记录
///
/// 包含时间戳、日志级别和消息内容
#[derive(Debug, Clone, Serialize)]
struct LogEntry {
  /// 时间戳（毫秒）
  timestamp_ms: u64,
  /// 日志级别
  level: LogLevel,
  /// 日志消息内容
  message: String,
}

impl LogEntry {
  /// 创建新的日志条目（自动记录当前时间戳）
  fn new(level: LogLevel, message: impl Into<String>) -> Self {
    Self {
      timestamp_ms: current_timestamp_ms(),
      level,
      message: message.into(),
    }
  }
}

/// 软件类型枚举
///
/// 标识不同的软件组件
#[derive(Debug, Clone, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
enum SoftwareKind {
  /// 模拟器（Emulator）
  Emulator,
  /// MAA 任务执行器
  Maa,
}

/// 软件运行阶段枚举
///
/// 表示软件当前的运行状态
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum SoftwarePhase {
  /// 未知状态
  Unknown,
  /// 空闲状态（特别用于 MAA）
  Idle,
  /// 启动中
  Starting,
  /// 运行中
  Running,
  /// 停止中
  Stopping,
  /// 已停止
  Stopped,
  /// 异常/错误状态
  Error,
}

/// 单个软件组件的运行状态快照
///
/// 记录某个时刻软件的阶段、消息和更新时间
#[derive(Debug, Clone, Serialize)]
struct SoftwareStatus {
  /// 软件类型
  kind: SoftwareKind,
  /// 当前运行阶段
  phase: SoftwarePhase,
  /// 最后一条状态消息
  last_message: Option<String>,
  /// 最后一次更新的时间戳（毫秒）
  last_updated_ms: u64,
}

impl SoftwareStatus {
  /// 创建指定类型和阶段的状态
  fn with_phase(kind: SoftwareKind, phase: SoftwarePhase) -> Self {
    Self {
      kind,
      phase,
      last_message: None,
      last_updated_ms: current_timestamp_ms(),
    }
  }
}

/// 应用全局状态管理
///
/// 包含运行时配置、日志缓冲区和软件状态映射
/// 使用 Mutex 确保线程安全
struct AppState {
  /// 运行时配置（从 TOML 或环境变量加载）
  config: AppConfig,
  /// 内部可变状态（使用 Mutex 保护）
  inner: Mutex<StateInner>,
}

/// 应用状态的内部可变部分
struct StateInner {
  /// 循环日志缓冲区（保持最多 MAX_MEMORY_LOGS 条记录）
  logs: VecDeque<LogEntry>,
  /// 各软件组件的当前状态映射
  statuses: HashMap<SoftwareKind, SoftwareStatus>,
}

impl AppState {
  /// 创建新的应用状态
  fn new(config: AppConfig) -> Self {
    let mut statuses = HashMap::new();
    // 初始化模拟器状态为 "已停止"
    statuses.insert(
      SoftwareKind::Emulator,
      SoftwareStatus::with_phase(SoftwareKind::Emulator, SoftwarePhase::Stopped),
    );
    // 初始化 MAA 状态为 "空闲"
    statuses.insert(
      SoftwareKind::Maa,
      SoftwareStatus::with_phase(SoftwareKind::Maa, SoftwarePhase::Idle),
    );

    Self {
      config,
      inner: Mutex::new(StateInner {
        logs: VecDeque::new(),
        statuses,
      }),
    }
  }

  /// 获取运行时配置的引用
  fn config(&self) -> &AppConfig {
    &self.config
  }

  /// 推送新日志到缓冲区，并触发前端事件
  ///
  /// - 将日志添加到循环缓冲区
  /// - 如果超过容量限制，自动删除最旧的日志
  /// - 通过 `backend://log` 事件通知前端
  fn push_log(&self, app: &AppHandle, level: LogLevel, message: impl Into<String>) {
    let entry = LogEntry::new(level, message);
    {
      let mut guard = self.inner.lock().expect("state poisoned");
      guard.logs.push_back(entry.clone());
      if guard.logs.len() > MAX_MEMORY_LOGS {
        guard.logs.pop_front();
      }
    }
    let _ = app.emit(LOG_EVENT, entry);
  }

  /// 获取当前日志快照（最多最后 200 条）
  fn logs_snapshot(&self) -> Vec<LogEntry> {
    let guard = self.inner.lock().expect("state poisoned");
    guard.logs.iter().cloned().collect()
  }

  /// 更新软件状态并通知前端
  ///
  /// # 参数
  /// * `app` - Tauri 应用句柄
  /// * `kind` - 软件类型
  /// * `phase` - 新的运行阶段
  /// * `message` - 状态消息（可选）
  ///
  /// # 返回值
  /// 返回更新后的状态快照
  fn update_status(
    &self,
    app: &AppHandle,
    kind: SoftwareKind,
    phase: SoftwarePhase,
    message: Option<String>,
  ) -> SoftwareStatus {
    let mut guard = self.inner.lock().expect("state poisoned");
    let status = guard
      .statuses
      .entry(kind.clone())
      .or_insert_with(|| SoftwareStatus::with_phase(kind.clone(), SoftwarePhase::Unknown));
    status.phase = phase;
    status.last_updated_ms = current_timestamp_ms();
    if let Some(msg) = message {
      status.last_message = Some(msg);
    }
    let snapshot = status.clone();
    drop(guard);
    // 通过 `backend://status` 事件通知前端
    let _ = app.emit(STATUS_EVENT, snapshot.clone());
    snapshot
  }

  /// 获取所有软件的当前状态快照
  fn statuses_snapshot(&self) -> Vec<SoftwareStatus> {
    let guard = self.inner.lock().expect("state poisoned");
    guard.statuses.values().cloned().collect()
  }
}

/// 指令操作类型枚举
///
/// 表示用户执行的不同操作，便于统一处理状态转换和日志记录
#[derive(Clone)]
enum ActionKind {
  /// 启动模拟器
  EmulatorStart,
  /// 停止模拟器
  EmulatorStop,
  /// 执行 MAA 任务
  MaaStartup,
}

impl ActionKind {
  /// 获取此操作涉及的软件类型
  fn target(&self) -> SoftwareKind {
    match self {
      ActionKind::EmulatorStart | ActionKind::EmulatorStop => SoftwareKind::Emulator,
      ActionKind::MaaStartup => SoftwareKind::Maa,
    }
  }

  /// 获取操作开始时的阶段
  fn start_phase(&self) -> SoftwarePhase {
    match self {
      ActionKind::EmulatorStart => SoftwarePhase::Starting,
      ActionKind::EmulatorStop => SoftwarePhase::Stopping,
      ActionKind::MaaStartup => SoftwarePhase::Running,
    }
  }

  /// 获取操作成功时的最终阶段
  fn success_phase(&self) -> SoftwarePhase {
    match self {
      ActionKind::EmulatorStart => SoftwarePhase::Running,
      ActionKind::EmulatorStop => SoftwarePhase::Stopped,
      ActionKind::MaaStartup => SoftwarePhase::Idle,
    }
  }
}

/// 指令执行的结果
///
/// 包含执行的命令、退出码、输出内容等信息，返回给前端
#[derive(Debug, Serialize)]
struct CommandOutcome {
  /// 命令的显示标签
  label: String,
  /// 完整的命令行
  command: String,
  /// 进程退出码
  exit_code: i32,
  /// 是否成功（exit_code == 0）
  success: bool,
  /// 标准输出内容
  stdout: String,
  /// 标准错误内容
  stderr: String,
}

/// Tauri 指令: 启动模拟器
///
/// 从配置中读取启动参数并执行 `podman start` 或 `docker start` 等命令
#[tauri::command]
async fn start_emulator(app_handle: AppHandle, state: State<'_, AppState>) -> Result<CommandOutcome, String> {
  let state_ref: &AppState = &state;
  let spec = state_ref.config().emulator_start.clone();
  execute_simple_action(&app_handle, state_ref, ActionKind::EmulatorStart, spec, "模拟器已启动").await
}

/// Tauri 指令: 停止模拟器
///
/// 从配置中读取停止参数并执行 `podman stop` 或 `docker stop` 等命令
#[tauri::command]
async fn stop_emulator(app_handle: AppHandle, state: State<'_, AppState>) -> Result<CommandOutcome, String> {
  let state_ref: &AppState = &state;
  let spec = state_ref.config().emulator_stop.clone();
  execute_simple_action(&app_handle, state_ref, ActionKind::EmulatorStop, spec, "模拟器已关闭").await
}

/// Tauri 指令: 执行 MAA 任务
///
/// 启动 MAA (`maa startup Official`)，包含额外的 Server 酱通知支持
#[tauri::command]
async fn run_maa_startup(app_handle: AppHandle, state: State<'_, AppState>) -> Result<CommandOutcome, String> {
  let state_ref: &AppState = &state;
  let spec = state_ref.config().maa_startup.clone();
  let label = spec.label.clone();
  let action = ActionKind::MaaStartup;
  let kind = action.target();
  state_ref.update_status(&app_handle, kind.clone(), action.start_phase(), Some("准备启动MAA任务".into()));
  state_ref.push_log(&app_handle, LogLevel::Info, format!("开始执行 {}", label));

  spawn_notification(&app_handle, "archMAA", "MAA服务准备启动");

  match run_configured_command(spec).await {
    Ok(outcome) => {
      if outcome.success {
        state_ref.update_status(&app_handle, kind.clone(), action.success_phase(), Some("MAA任务已完成".into()));
        state_ref.push_log(&app_handle, LogLevel::Info, format!("{} 完成", label));
        spawn_notification(&app_handle, "archMAA", "MAA运行完毕");
        Ok(outcome)
      } else {
        let message = format!("{} 失败, 退出码 {}", label, outcome.exit_code);
        state_ref.update_status(&app_handle, kind, SoftwarePhase::Error, Some(message.clone()));
        state_ref.push_log(&app_handle, LogLevel::Error, message.clone());
        Err(message)
      }
    }
    Err(err) => {
      state_ref.update_status(&app_handle, kind, SoftwarePhase::Error, Some(err.clone()));
      state_ref.push_log(&app_handle, LogLevel::Error, err.clone());
      Err(err)
    }
  }
}

/// Tauri 指令: 获取软件状态快照
///
/// 前端调用此指令获取当前所有软件的状态（模拟器和 MAA）
#[tauri::command]
fn fetch_status(state: State<'_, AppState>) -> Vec<SoftwareStatus> {
  state.statuses_snapshot()
}

/// Tauri 指令: 获取日志快照
///
/// 前端调用此指令获取最多 200 条最新日志记录
#[tauri::command]
fn fetch_logs(state: State<'_, AppState>) -> Vec<LogEntry> {
  state.logs_snapshot()
}

/// 通用指令执行函数
///
/// 处理模拟器启动/停止等简单指令的完整流程：
/// 1. 更新状态为 "执行中"
/// 2. 执行命令
/// 3. 根据结果更新状态为成功或错误
/// 4. 记录日志
async fn execute_simple_action(
  app_handle: &AppHandle,
  state: &AppState,
  action: ActionKind,
  spec: CommandConfig,
  success_message: &str,
) -> Result<CommandOutcome, String> {
  let kind = action.target();
  let label = spec.label.clone();
  let command_preview = if spec.args.is_empty() {
    spec.program.clone()
  } else {
    format!("{} {}", spec.program, spec.args.join(" "))
  };
  // 更新状态为执行中
  state.update_status(
    app_handle,
    kind.clone(),
    action.start_phase(),
    Some(format!("{} 执行中", label)),
  );
  // 记录执行日志
  state.push_log(
    app_handle,
    LogLevel::Info,
    format!("执行 {} => {}", label, command_preview),
  );

  // 执行命令
  match run_configured_command(spec).await {
    Ok(outcome) => {
      if outcome.success {
        // 成功：更新状态为成功状态
        state.update_status(app_handle, kind, action.success_phase(), Some(success_message.to_string()));
        state.push_log(app_handle, LogLevel::Info, format!("{} 完成", label));
        // 即使成功也记录输出内容（便于调试）
        if !outcome.stdout.is_empty() {
          state.push_log(app_handle, LogLevel::Info, format!("[STDOUT] {}", outcome.stdout));
        }
        if !outcome.stderr.is_empty() {
          state.push_log(app_handle, LogLevel::Warn, format!("[STDERR] {}", outcome.stderr));
        }
        Ok(outcome)
      } else {
        // 失败：更新状态为错误，记录详细的错误日志
        let message = format!("{} 失败, 退出码 {}", label, outcome.exit_code);
        state.update_status(app_handle, kind, SoftwarePhase::Error, Some(message.clone()));
        state.push_log(app_handle, LogLevel::Error, message.clone());
        
        // 记录详细的 STDOUT 内容
        if !outcome.stdout.is_empty() {
          state.push_log(app_handle, LogLevel::Error, format!("[STDOUT] {}", outcome.stdout));
        } else {
          state.push_log(app_handle, LogLevel::Error, "[STDOUT] (无输出)".to_string());
        }
        
        // 记录详细的 STDERR 内容（这通常包含错误消息）
        if !outcome.stderr.is_empty() {
          state.push_log(app_handle, LogLevel::Error, format!("[STDERR] {}", outcome.stderr));
        } else {
          state.push_log(app_handle, LogLevel::Error, "[STDERR] (无输出)".to_string());
        }
        
        Err(message)
      }
    }
    Err(err) => {
      // 执行异常：更新状态为错误
      state.update_status(app_handle, kind, SoftwarePhase::Error, Some(err.clone()));
      state.push_log(app_handle, LogLevel::Error, err.clone());
      Err(err)
    }
  }
}

/// 执行配置化的系统命令
///
/// 在后台线程池中运行命令，捕获输出和退出码
/// 支持 sudo 提权、环境变量注入和工作目录设置
async fn run_configured_command(spec: CommandConfig) -> Result<CommandOutcome, String> {
  let command_display = if spec.args.is_empty() {
    spec.program.clone()
  } else {
    format!("{} {}", spec.program, spec.args.join(" "))
  };
  // 展开工作目录路径（支持 ~/）
  let working_dir = spec.working_dir.clone().map(expand_path);
  let env_vars: Vec<(String, String)> = spec.env.clone().into_iter().collect();
  let label = spec.label.clone();
  let program = spec.program.clone();
  let args = spec.args.clone();
  let requires_sudo = spec.requires_sudo;

  // 在阻塞线程池中执行命令（避免阻塞异步运行时）
  let output = tauri::async_runtime::spawn_blocking(move || {
    // 根据是否需要 sudo 选择执行方式
    if requires_sudo && !is_root() {
      // 尝试第一阶段：使用 sudo -n（非交互，如已缓存密码）
      let mut sudo_args = vec!["-n".into(), program.clone()];
      sudo_args.extend(args.iter().cloned());
      let result = execute_command_internal("sudo", &sudo_args, &working_dir, &env_vars);
      
      // 如果失败且是密码需求，尝试第二阶段：使用 pkexec（GUI 密码对话框）
      let stderr_str = String::from_utf8_lossy(&result.stderr);
      if !result.status.success() && (stderr_str.contains("password") || stderr_str.contains("a password is required")) {
        // 使用 pkexec 显示 GUI 密码对话框
        let mut pkexec_args = vec![program.clone()];
        pkexec_args.extend(args.clone());
        return execute_command_internal("pkexec", &pkexec_args, &working_dir, &env_vars);
      }
      result
    } else {
      // 直接执行，不需要 sudo
      execute_command_internal(&program, &args, &working_dir, &env_vars)
    }
  })
  .await
  .map_err(|err| format!("指令执行线程崩溃: {err}"))?;

  // 将 STDOUT/STDERR 从字节转换为字符串
  let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
  let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
  let exit_code = output.status.code().unwrap_or(-1);
  let success = output.status.success();

  Ok(CommandOutcome {
    label,
    command: command_display,
    exit_code,
    success,
    stdout,
    stderr,
  })
}

/// 执行单条命令的内部函数
/// 
/// 实际上执行 Command 的逻辑封装
fn execute_command_internal(
  program: &str,
  args: &[String],
  working_dir: &Option<PathBuf>,
  env_vars: &[(String, String)],
) -> std::process::Output {
  let mut command = Command::new(program);
  for arg in args {
    command.arg(arg);
  }
  // 如果指定了工作目录，切换到该目录
  if let Some(dir) = working_dir {
    command.current_dir(dir);
  }
  // 注入环境变量
  for (key, value) in env_vars {
    command.env(key, value);
  }
  // 捕获标准输出和标准错误
  command.stdout(Stdio::piped()).stderr(Stdio::piped());
  // 执行并返回输出
  command.output().unwrap_or_else(|e| {
    std::process::Output {
      status: Command::new("false").output().unwrap().status,
      stdout: Vec::new(),
      stderr: format!("Failed to execute command: {}", e).into_bytes(),
    }
  })
}


/// 展开路径中的 `~/` 前缀
///
/// 将 `~/something` 转换为 `/home/user/something`
fn expand_path(path: String) -> PathBuf {
  if let Some(stripped) = path.strip_prefix("~/") {
    if let Ok(home) = env::var("HOME") {
      return PathBuf::from(home).join(stripped);
    }
  }
  PathBuf::from(path)
}

/// 获取当前时间戳（毫秒）
fn current_timestamp_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or(Duration::from_secs(0))
    .as_millis() as u64
}

/// 检查当前进程是否以 root 身份运行
///
/// 使用 libc 的 geteuid() 系统调用检查有效用户 ID
fn is_root() -> bool {
  unsafe { libc::geteuid() == 0 }
}

/// 异步发送 Server 酱推送通知
///
/// 从环境变量中读取 SENDKEY，如果存在则发送推送消息
/// 推送在后台异步执行，不阻塞主逻辑
fn spawn_notification(app_handle: &AppHandle, text: &str, desp: &str) {
  if let Ok(sendkey) = env::var("SENDKEY") {
    let app = app_handle.clone();
    let text = text.to_string();
    let desp = desp.to_string();
    // 在异步运行时中发送推送（不阻塞调用者）
    tauri::async_runtime::spawn(async move {
      let state = app.state::<AppState>();
      let state_ref: &AppState = &state;
      match send_server_chan(&sendkey, &text, &desp).await {
        Ok(_) => state_ref.push_log(&app, LogLevel::Info, format!("Server 酱推送成功: {desp}")),
        Err(err) => state_ref.push_log(&app, LogLevel::Warn, format!("Server 酱推送失败: {err}")),
      }
    });
  }
}
