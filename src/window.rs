//! 桌宠窗口模块：透明无边框、置顶、可拖动、单进程多窗。
//!
//! Rust 选型（路线图第二阶段落地）：
//! - 窗口与事件循环：`winit`（跨平台，支持 Linux Wayland/X11、Windows、macOS）
//! - 透明与置顶：winit `WindowAttributes::with_transparent(true)` + 平台特定扩展
//! - 帧渲染：`softbuffer`（CPU 软渲染起步）或 `wgpu`（后期硬件加速）
//!
//! 对应原项目 `pet/window.py`（约 4600 行，处于"只许瘦不许胖"公约下）。
//! 重构时按职责拆分为：window / drag / multiwin / edge_probe 等子模块。

/// 窗口配置占位。
#[derive(Debug, Clone)]
pub struct PetWindowConfig {
    /// 是否置顶
    pub always_on_top: bool,
    /// 是否透明背景
    pub transparent: bool,
    /// 初始缩放（100 = 1.0）
    pub scale: f32,
}

impl Default for PetWindowConfig {
    fn default() -> Self {
        Self {
            always_on_top: true,
            transparent: true,
            scale: 1.0,
        }
    }
}

/// 桌宠窗口占位。第二阶段接入 winit 后补全生命周期与事件处理。
#[derive(Debug, Clone)]
pub struct PetWindow {
    pub config: PetWindowConfig,
}

impl PetWindow {
    pub fn new(config: PetWindowConfig) -> Self {
        Self { config }
    }
}
