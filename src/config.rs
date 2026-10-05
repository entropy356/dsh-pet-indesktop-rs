//! 配置模块：数据模型、加载/保存、多开 slot 作用域。
//!
//! 对应原项目 `config.json` / `config-slot-N.json` 机制：
//! - 每窗独立项（形象/位置/缩放等）存 `config-slot-N.json`
//! - 进程级共享项（托盘/多开等）以主桌宠 `config.json` 为准
//!
//! 字段命名对齐原项目 `pet/config.py` 的默认值定义（issue #3）。
//! JSON 由 `serde_json` 编解码，所有字段带 `#[serde(default)]`，
//! 旧版本/缺字段的配置文件也能正常读取。

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 原项目 `catalog.DEFAULT_CHARACTER`（「深深」）。
pub const DEFAULT_CHARACTER: &str = "shenshen";
/// 原项目 `catalog.DEFAULT_SCALE`。
pub const DEFAULT_SCALE: f64 = 0.72;
/// 配置 schema 版本，对齐原项目 `self.data["version"]`。
pub const CONFIG_VERSION: u32 = 4;

/// 每窗独立配置（形象、位置、缩放）。
///
/// 字段名与原项目 `config.json` 保持一致：
/// - `character` 形象 id（原 `catalog.DEFAULT_CHARACTER`）
/// - `rx` / `ry` 窗口位置（像素，`None` 表示未记录、由窗口层居中）
/// - `screen_name` 所在屏幕名（多屏）
/// - `facing` 朝向（`"left"` / `"right"`）
/// - `scale` 缩放（原 `catalog.DEFAULT_SCALE`）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SlotConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_character")]
    pub character: String,
    #[serde(default)]
    pub rx: Option<f64>,
    #[serde(default)]
    pub ry: Option<f64>,
    #[serde(default)]
    pub screen_name: Option<String>,
    #[serde(default = "default_facing")]
    pub facing: String,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default = "default_true")]
    pub on_top: bool,
}

fn default_version() -> u32 {
    CONFIG_VERSION
}
fn default_character() -> String {
    DEFAULT_CHARACTER.to_string()
}
fn default_facing() -> String {
    "left".to_string()
}
fn default_scale() -> f64 {
    DEFAULT_SCALE
}
fn default_true() -> bool {
    true
}

impl Default for SlotConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            character: DEFAULT_CHARACTER.to_string(),
            rx: None,
            ry: None,
            screen_name: None,
            facing: "left".to_string(),
            scale: DEFAULT_SCALE,
            on_top: true,
        }
    }
}

/// 进程级共享配置（托盘、多开开关等）。
///
/// 原项目 config.json 无独立托盘键（托盘常驻），`tray_enabled` 为
/// Rust 端新增键；`multi_window` 对应原「单进程多开」语义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedConfig {
    /// 系统托盘开关。
    #[serde(default = "default_true")]
    pub tray_enabled: bool,
    /// 是否启用单进程多开。
    #[serde(default)]
    pub multi_window: bool,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            tray_enabled: true,
            multi_window: false,
        }
    }
}

/// 配置读写错误：解析失败携带文件与原因，不 panic。
#[derive(Debug)]
pub enum ConfigError {
    /// 文件系统错误（读/写/重命名）。
    Io { path: PathBuf, source: io::Error },
    /// JSON 解析失败，携带 reason。
    Parse { path: PathBuf, reason: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io { path, source } => {
                write!(f, "配置文件 IO 失败 {}: {source}", path.display())
            }
            ConfigError::Parse { path, reason } => {
                write!(f, "配置文件解析失败 {}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConfigError::Io { source, .. } => Some(source),
            ConfigError::Parse { .. } => None,
        }
    }
}

/// 计算指定 slot 的配置文件路径（slot 0 为主配置）。
pub fn config_path(config_dir: &Path, slot: u8) -> PathBuf {
    if slot == 0 {
        config_dir.join("config.json")
    } else {
        config_dir.join(format!("config-slot-{slot}.json"))
    }
}

/// 按平台定位默认配置目录（应用子目录 `dsh-pet-indesktop-rs`）：
/// - Windows: `%APPDATA%`（回退 `%USERPROFILE%\AppData\Roaming`）
/// - Linux: `$XDG_CONFIG_HOME` 或 `~/.config`
/// - macOS: `~/Library/Application Support`
pub fn config_dir() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        home_dir().map(|h| h.join("Library").join("Application Support"))
    } else if cfg!(target_os = "windows") {
        std::env::var("APPDATA")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join("AppData").join("Roaming")))
    } else {
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|v| v.starts_with('/'))
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".config")))
    }
    .unwrap_or_else(|| PathBuf::from("."));
    base.join("dsh-pet-indesktop-rs")
}

fn home_dir() -> Option<PathBuf> {
    if cfg!(target_os = "windows") {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    } else {
        std::env::var("HOME")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    }
}

/// 加载 slot 配置：文件缺失 → 默认值；存在但解析失败 → 带 reason 的错误。
pub fn load_or_default(path: &Path) -> Result<SlotConfig, ConfigError> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| ConfigError::Parse {
            path: path.to_path_buf(),
            reason: e.to_string(),
        }),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(SlotConfig::default()),
        Err(e) => Err(ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// 首次运行语义：配置文件缺失时创建父目录并写入默认 `config.json` 后返回默认值；
/// 已存在则等价于 [`load_or_default`]。
pub fn load_or_create_default(path: &Path) -> Result<SlotConfig, ConfigError> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        let default = SlotConfig::default();
        save(path, &default)?;
        return Ok(default);
    }
    load_or_default(path)
}

/// 原子写保存：先写同目录临时文件，再 rename 覆盖目标。
pub fn save(path: &Path, config: &SlotConfig) -> Result<(), ConfigError> {
    let text = serde_json::to_string_pretty(config).map_err(|e| ConfigError::Parse {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.json")
    ));

    let write = || -> Result<(), ConfigError> {
        fs::write(&tmp, text.as_bytes()).map_err(|e| ConfigError::Io {
            path: tmp.clone(),
            source: e,
        })?;
        #[cfg(target_os = "windows")]
        if path.exists() {
            fs::remove_file(path).map_err(|e| ConfigError::Io {
                path: path.to_path_buf(),
                source: e,
            })?;
        }
        fs::rename(&tmp, path).map_err(|e| ConfigError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    };

    let result = write();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_paths() {
        let dir = PathBuf::from("/tmp/x");
        assert_eq!(config_path(&dir, 0), PathBuf::from("/tmp/x/config.json"));
        assert_eq!(
            config_path(&dir, 2),
            PathBuf::from("/tmp/x/config-slot-2.json")
        );
    }

    #[test]
    fn load_missing_file_returns_default() {
        let path =
            std::env::temp_dir().join(format!("dsh-cfg-missing-{}.json", std::process::id()));
        let cfg = load_or_default(&path).expect("缺失文件应返回默认值");
        assert_eq!(cfg, SlotConfig::default());
        assert_eq!(cfg.character, DEFAULT_CHARACTER);
        assert!((cfg.scale - DEFAULT_SCALE).abs() < f64::EPSILON);
    }

    #[test]
    fn roundtrip_save_load() {
        let dir = std::env::temp_dir().join(format!("dsh-cfg-rt-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = config_path(&dir, 3);

        let cfg = SlotConfig {
            character: "big_blue_fat_fish".to_string(),
            rx: Some(120.0),
            ry: Some(240.0),
            facing: "right".to_string(),
            scale: 1.5,
            on_top: false,
            ..SlotConfig::default()
        };

        save(&path, &cfg).expect("保存应成功");
        assert!(!path
            .with_file_name(format!(
                ".{}.tmp",
                path.file_name().unwrap().to_str().unwrap()
            ))
            .exists());

        let loaded = load_or_default(&path).expect("回读应成功");
        assert_eq!(loaded, cfg);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bad_json_returns_parse_error() {
        let dir = std::env::temp_dir().join(format!("dsh-cfg-bad-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = config_path(&dir, 1);
        fs::write(&path, "{\"character\": 123,").unwrap();

        match load_or_default(&path) {
            Err(ConfigError::Parse { path: p, reason }) => {
                assert_eq!(p, path);
                assert!(!reason.is_empty());
            }
            other => panic!("应返回 Parse 错误，实际 {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn partial_json_fills_defaults() {
        let cfg: SlotConfig = serde_json::from_str("{\"rx\": 10.0}").unwrap();
        assert_eq!(cfg.rx, Some(10.0));
        assert_eq!(cfg.character, DEFAULT_CHARACTER);
        assert_eq!(cfg.version, CONFIG_VERSION);
    }

    #[test]
    fn shared_config_compat() {
        let cfg: SharedConfig = serde_json::from_str("{}").unwrap();
        assert!(cfg.tray_enabled);
        assert!(!cfg.multi_window);
    }

    #[test]
    fn first_run_creates_dir_and_default_config() {
        let dir = std::env::temp_dir().join(format!("dsh-cfg-first-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        let path = config_path(&dir, 0);
        assert!(!path.exists());

        let cfg = load_or_create_default(&path).expect("首次运行应自动创建");
        assert_eq!(cfg, SlotConfig::default());
        assert!(path.exists());

        let on_disk: SlotConfig =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(on_disk, SlotConfig::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn config_dir_has_app_subdir() {
        let dir = config_dir();
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some("dsh-pet-indesktop-rs")
        );
    }
}
