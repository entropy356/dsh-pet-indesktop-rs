//! 配置模块：加载/保存、多开 slot 作用域。
//!
//! 对应原项目 `config.json` / `config-slot-N.json` 机制：
//! - 每窗独立项（形象/位置/聊天等）存 `config-slot-N.json`
//! - 进程级共享项（托盘/共享解码/Agent 联动等）以主桌宠 `config.json` 为准
//!
//! Rust 选型（路线图第二阶段落地）：`serde` + `toml`（或保持 JSON 用 `serde_json`）。

use std::path::PathBuf;

/// 进程级共享配置占位。
#[derive(Debug, Clone)]
pub struct SharedConfig {
    /// 是否启用单进程多开
    pub multi_window: bool,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self { multi_window: false }
    }
}

/// 计算指定 slot 的配置文件路径（slot 0 为主配置）。
pub fn config_path(config_dir: &PathBuf, slot: u8) -> PathBuf {
    if slot == 0 {
        config_dir.join("config.json")
    } else {
        config_dir.join(format!("config-slot-{slot}.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_paths() {
        let dir = PathBuf::from("/tmp/x");
        assert_eq!(config_path(&dir, 0), PathBuf::from("/tmp/x/config.json"));
        assert_eq!(config_path(&dir, 2), PathBuf::from("/tmp/x/config-slot-2.json"));
    }
}
