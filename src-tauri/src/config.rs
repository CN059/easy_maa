//! 配置模块：处理应用程序的命令配置
//! 
//! 本模块负责加载和解析应用配置，支持以下方式：
//! 1. 从 TOML 文件加载配置（生产和调试模式均支持）
//! 2. 从环境变量读取默认配置
//! 3. 使用硬编码的默认值作为最后的备选方案
//! 
//! TOML 配置文件路径查找顺序：
//! - 优先级1: EASY_MAA_CONFIG 环境变量指定的路径
//! - 优先级2 (debug模式): ~/.config/easy_maa/runtime.toml
//! - 优先级3: ~/.config/easy_maa/easy_maa.toml
//! - 优先级4: 使用环境变量 + 默认值的组合

use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// 生产环境下的默认配置文件路径
const DEFAULT_CONFIG_PATH_PROD: &str = ".config/easy_maa/easy_maa.toml";

/// Debug 环境下的配置文件路径（优先级更高）
#[cfg(debug_assertions)]
const DEFAULT_CONFIG_PATH_DEBUG: &str = ".config/easy_maa/runtime.toml";

/// 单条命令的配置结构体
/// 
/// 存储一条具体指令的所有执行参数，包括：
/// - 程序名称和参数
/// - 是否需要 sudo 权限
/// - 工作目录和环境变量
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct CommandConfig {
    /// 命令的显示标签，用于 UI 和日志输出
    pub label: String,
    
    /// 可执行程序的名称或路径（如 "podman", "maa", "/usr/bin/docker"）
    pub program: String,
    
    /// 传给程序的参数列表
    pub args: Vec<String>,
    
    /// 是否需要使用 sudo 提权执行
    /// 当为 true 时，实际执行 `sudo -n <program> <args>`
    pub requires_sudo: bool,
    
    /// 指令执行的工作目录（支持 ~/ 展开）
    pub working_dir: Option<String>,
    
    /// 额外的环境变量（会在执行时注入）
    pub env: HashMap<String, String>,
}

impl Default for CommandConfig {
    fn default() -> Self {
        Self {
            label: "custom".into(),
            program: "true".into(),
            args: Vec::new(),
            requires_sudo: false,
            working_dir: None,
            env: HashMap::new(),
        }
    }
}

/// 应用级别的配置结构体
/// 
/// 包含所有主要操作的命令配置：
/// - 模拟器启动
/// - 模拟器停止
/// - MAA 任务执行
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    /// 启动模拟器的命令配置
    pub emulator_start: CommandConfig,
    
    /// 停止模拟器的命令配置
    pub emulator_stop: CommandConfig,
    
    /// 执行 MAA 任务（通常是 `maa startup Official`）的命令配置
    pub maa_startup: CommandConfig,
}

impl Default for AppConfig {
    /// 生成默认配置
    /// 
    /// 优先从环境变量读取，其次使用硬编码的默认值
    /// 环境变量列表：
    /// - CONTAINER_NAME: 模拟器容器名称（默认: "maa-container"）
    /// - EMULATOR_PROGRAM: 模拟器程序名称（默认: "podman"）
    /// - EMULATOR_NEEDS_SUDO: 模拟器是否需要 sudo（默认: false）
    /// - MAA_BIN: MAA 程序路径（默认: "maa"）
    /// - MAA_PROFILE: MAA 配置文件名（默认: "Official"）
    fn default() -> Self {
        // 从环境变量读取容器名称
        let container_name = env::var("CONTAINER_NAME")
            .unwrap_or_else(|_| "maa-container".into());

        // 从环境变量读取模拟器程序（通常是 podman 或 docker）
        let emulator_program = env::var("EMULATOR_PROGRAM")
            .unwrap_or_else(|_| "podman".into());

        // 从环境变量读取 MAA 的配置文件名
        let maa_profile = env::var("MAA_PROFILE")
            .unwrap_or_else(|_| "Official".into());

        // 从环境变量读取 MAA 二进制文件路径
        let maa_program = env::var("MAA_BIN")
            .unwrap_or_else(|_| "maa".into());

        // 从环境变量判断模拟器是否需要 sudo（支持 "1", "true", "TRUE" 等格式）
        // 如果设置了 SKIP_SUDO_VALIDATION=1，则强制 requires_sudo=false（允许无 NOPASSWD sudo 的环境）
        let skip_sudo = env::var("SKIP_SUDO_VALIDATION")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        
        let emulator_requires_sudo = if skip_sudo {
            false
        } else {
            env::var("EMULATOR_NEEDS_SUDO")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
        };

        Self {
            // 启动模拟器的命令配置
            emulator_start: CommandConfig {
                label: "启动模拟器".into(),
                program: emulator_program.clone(),
                args: vec!["start".into(), container_name.clone()],
                requires_sudo: emulator_requires_sudo,
                ..Default::default()
            },
            // 停止模拟器的命令配置
            emulator_stop: CommandConfig {
                label: "关闭模拟器".into(),
                program: emulator_program,
                args: vec!["stop".into(), container_name],
                requires_sudo: emulator_requires_sudo,
                ..Default::default()
            },
            // MAA 启动命令配置（通常不需要 sudo）
            maa_startup: CommandConfig {
                label: "MAA 启动".into(),
                program: maa_program,
                args: vec!["startup".into(), maa_profile],
                requires_sudo: false,
                ..Default::default()
            },
        }
    }
}

impl AppConfig {
    /// 加载应用配置
    /// 
    /// 按以下优先级加载配置：
    /// 1. 如果文件存在，加载 TOML 配置文件
    /// 2. 否则，使用环境变量 + 默认值的组合
    /// 
    /// # 返回值
    /// 总是返回一个有效的 AppConfig 实例（如果文件解析失败会使用默认值）
    pub fn load() -> Self {
        if let Some(path) = Self::resolve_path() {
            if path.exists() {
                log::info!("尝试从配置文件加载: {}", path.display());
                match Self::read_from_path(&path) {
                    Ok(cfg) => {
                        log::info!("配置文件加载成功");
                        return cfg;
                    }
                    Err(err) => {
                        log::warn!("配置文件解析失败，使用默认配置: {err}");
                    }
                }
            } else {
                log::info!("配置文件不存在: {}", path.display());
            }
        } else {
            log::info!("未检测到配置路径，将使用环境变量 + 默认值");
        }

        Self::default()
    }

    /// 从 TOML 文件读取配置
    /// 
    /// # 参数
    /// * `path` - 配置文件的路径
    /// 
    /// # 返回值
    /// 返回解析后的 AppConfig，或返回错误信息
    fn read_from_path(path: &Path) -> Result<Self, String> {
        // 读取文件内容
        let content = fs::read_to_string(path)
            .map_err(|err| format!("无法读取配置文件 {}: {err}", path.display()))?;
        
        // 使用 toml 库解析 TOML 格式
        toml::from_str(&content)
            .map_err(|err| format!("TOML 配置文件格式错误 {}: {err}", path.display()))
    }

    /// 解析配置文件路径
    /// 
    /// 查找路径的优先级：
    /// 1. EASY_MAA_CONFIG 环境变量（如果设置）
    /// 2. ~/.config/easy_maa/runtime.toml（仅 debug 构建）
    /// 3. ~/.config/easy_maa/easy_maa.toml
    /// 
    /// # 返回值
    /// 如果可以确定 HOME 目录，返回 Some(path)，否则返回 None
    fn resolve_path() -> Option<PathBuf> {
        // 优先级1: 检查 EASY_MAA_CONFIG 环境变量
        if let Ok(custom) = env::var("EASY_MAA_CONFIG") {
            log::debug!("使用 EASY_MAA_CONFIG 环境变量指定的配置路径: {}", custom);
            return Some(PathBuf::from(custom));
        }

        // 获取用户 HOME 目录
        let home = env::var("HOME").ok()?;
        let home_path = PathBuf::from(home);

        // 优先级2: Debug 构建时先检查 runtime.toml（用于快速迭代开发）
        #[cfg(debug_assertions)]
        {
            let debug_path = home_path.join(DEFAULT_CONFIG_PATH_DEBUG);
            if debug_path.exists() {
                log::debug!("Debug 构建模式，使用 runtime.toml: {}", debug_path.display());
                return Some(debug_path);
            }
        }

        // 优先级3: 生产环境配置路径
        let prod_path = home_path.join(DEFAULT_CONFIG_PATH_PROD);
        log::debug!("使用生产环境配置路径: {}", prod_path.display());
        Some(prod_path)
    }
}
