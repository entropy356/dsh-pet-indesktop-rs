//! dsh-pet-indesktop-rs —— 库入口。
//!
//! Rust 重构版桌面宠物（原项目：https://github.com/MerZlin/dsh-pet-indesktop）。
//! 目标：透明无边框、置顶、可拖动的桌宠窗口，支持角色切换、动画播放、
//! 系统托盘、多开与可选 AI 对话能力，覆盖 Windows / Linux / macOS。
//!
//! 模块划分：
//! - [`window`]：桌宠窗口（透明、置顶、拖动、多窗）
//! - [`animation`]：动画播放（解码链 + 帧调度）
//! - [`physics`]：纯物理/碰撞逻辑层（不依赖 GUI）
//! - [`sim`]：模拟控制器（窗口状态机 × 物理步进的纯逻辑接线，issue #11）
//! - [`tray`]：系统托盘与右键菜单
//! - [`config`]：配置加载/保存与多开 slot 作用域
//!
//! 路线图见 README.md。骨架阶段二进制仅打印模块划分。

pub mod animation;
pub mod config;
pub mod decode;
pub mod physics;
pub mod sim;
pub mod tray;
pub mod window;
