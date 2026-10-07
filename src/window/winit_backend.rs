//! winit 后端：[`WindowBackend`](super::backend::WindowBackend) trait 的
//! winit 实现（issue #9，第二阶段起步）。
//!
//! 职责边界：
//! - 仅做 winit `Window` 调用的轻量转接，全部方法保持非阻塞语义
//!   （对应 winit `Window` 方法的事件驱动语义，不做同步等待）。
//! - 不持有状态机、不关心渲染内容；帧绘制细节另行拆 issue，本文件只负责
//!   `request_redraw` 转接与窗口属性构建。
//! - 窗口创建参数按 issue 要求：透明 + 无边框 + 置顶，初始尺寸/位置从
//!   config（[`SlotConfig`]）读取。
//!
//! 单元测试在无显示环境下运行（CI ubuntu 无 display），因此只测纯函数
//! 构建逻辑（尺寸/位置换算、属性构建），不实际创建 winit 窗口。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::window::{CursorGrabMode, Window, WindowAttributes, WindowLevel};

use crate::config::SlotConfig;

use super::backend::WindowBackend;

/// winit `Window` 的 [`WindowBackend`] 适配器。
///
/// `Window` 内部为 `Arc` 型句柄（Send + Sync，所有方法线程安全且非阻塞），
/// 因此本结构直接持句柄转发调用。
#[derive(Debug, Clone)]
pub struct WinitBackend {
    window: Arc<Window>,
    /// 拖拽期指针限制状态（issue #36）：`set_position` 移动窗口后据此
    /// 重申 Confined，保证限制矩形跟随窗口。原子量即可——纯开关语义。
    confined: Arc<AtomicBool>,
}

impl WinitBackend {
    /// 用已创建的 winit 窗口句柄构造后端（由事件循环侧 `resumed` 创建）。
    pub fn new(window: Arc<Window>) -> Self {
        Self {
            window,
            confined: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 只读访问底层 winit 窗口（事件循环侧需要 id / 监视器等信息时使用）。
    pub fn window(&self) -> &Arc<Window> {
        &self.window
    }
}

impl WindowBackend for WinitBackend {
    fn show(&self) {
        self.window.set_visible(true);
    }

    fn hide(&self) {
        self.window.set_visible(false);
    }

    fn set_position(&self, x: i32, y: i32) {
        // PetWindow 的坐标语义为屏幕像素（对应原项目 config.json 的
        // rx / ry），因此用 PhysicalPosition，避免与 DPI 缩放换算混淆。
        self.window.set_outer_position(PhysicalPosition::new(x, y));
        // 拖拽期重申限制（issue #36）：Windows 的 ClipCursor 矩形在每次
        // set_cursor_grab(Confined) 时按窗口**当前**矩形重设（winit
        // 0.30.13 window_state.rs::refresh_os_cursor 无同模式 early-return），
        // 不重申则限制区域停留在限住瞬间的旧窗口矩形，拖拽范围被冻结。
        // X11 / Wayland 同模式调用为幂等轻操作，无副作用。
        if self.confined.load(Ordering::Relaxed) {
            let _ = self.apply_cursor_grab(CursorGrabMode::Confined);
        }
    }

    fn set_always_on_top(&self, on: bool) {
        let level = if on {
            WindowLevel::AlwaysOnTop
        } else {
            WindowLevel::Normal
        };
        self.window.set_window_level(level);
    }

    fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// 拖拽期把指针限制在窗口内（issue #36，候选 1：winit Confined）。
    ///
    /// 平台差异（winit 0.30.13 源码级核实）：
    /// - **Windows**：`ClipCursor`，每次调用按当前窗口矩形重设；
    /// - **X11**：`XGrabPointer` 全局捕获——事件送达不依赖指针在窗内，
    ///   脱离问题直接消除；confine 区域随窗口动态求值；
    /// - **Wayland**：`zwp_pointer_constraints` Confined；
    /// - **macOS**：Confined 返回 `NotSupported`——**静默回退**到无限制
    ///   现状；AppKit 将拖拽事件隐式路由给 `mouseDown` 窗口，指针脱离
    ///   窗口后事件仍送达，行为无劣化。
    ///
    /// 失败一律静默忽略（best-effort，见 trait 文档），不向调用方传播。
    fn set_cursor_confined(&self, on: bool) {
        self.confined.store(on, Ordering::Relaxed);
        let mode = if on {
            CursorGrabMode::Confined
        } else {
            CursorGrabMode::None
        };
        let _ = self.apply_cursor_grab(mode);
    }
}

impl WinitBackend {
    /// 统一的 grab 应用入口：单点消化 winit 的 `Result`（best-effort）。
    fn apply_cursor_grab(&self, mode: CursorGrabMode) -> Result<(), winit::error::ExternalError> {
        self.window.set_cursor_grab(mode)
    }
}

/// 基础窗口尺寸（逻辑像素，占位值）。真实尺寸待第二阶段引入角色素材
/// 首帧后按视频分辨率 × scale 决定（帧渲染细节另拆 issue）。
pub const BASE_WINDOW_SIZE: (f64, f64) = (300.0, 300.0);

/// 按缩放换算初始窗口尺寸（逻辑像素）。原项目窗口尺寸 = 素材尺寸 × scale，
/// 素材引入前用 [`BASE_WINDOW_SIZE`] 占位。
pub fn scaled_window_size(scale: f64) -> (f64, f64) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    (BASE_WINDOW_SIZE.0 * scale, BASE_WINDOW_SIZE.1 * scale)
}

/// 从 config 换算初始位置：`rx`/`ry` 都记录过才生效（像素，原点左上），
/// 任一缺失返回 `None`，由窗口层决定放置策略（原项目行为：未记录则居中）。
pub fn initial_position(rx: Option<f64>, ry: Option<f64>) -> Option<(i32, i32)> {
    match (rx, ry) {
        (Some(x), Some(y)) => Some((x as i32, y as i32)),
        _ => None,
    }
}

/// 从 [`SlotConfig`] 构建窗口属性：透明、无边框、按 config 决定置顶，
/// 初始尺寸 = 基础尺寸 × scale，初始位置取 config 记录值（可能为 None）。
pub fn window_attributes(config: &SlotConfig) -> WindowAttributes {
    let (w, h) = scaled_window_size(config.scale);
    let mut attrs = Window::default_attributes()
        .with_transparent(true)
        .with_decorations(false)
        .with_resizable(false)
        .with_inner_size(LogicalSize::new(w, h));
    if config.on_top {
        attrs = attrs.with_window_level(WindowLevel::AlwaysOnTop);
    }
    if let Some((x, y)) = initial_position(config.rx, config.ry) {
        attrs = attrs.with_position(PhysicalPosition::new(x, y));
    }
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 编译期断言：`WinitBackend` 实现 `WindowBackend`（无需构造实例，
    /// 无显示环境也能验证 trait 实现存在）。
    const _: () = {
        const fn _assert_backend() {
            fn takes(_b: &dyn WindowBackend) {}
            fn check(b: &WinitBackend) {
                takes(b);
            }
            let _ = check;
        }
        _assert_backend();
    };

    #[test]
    fn scaled_window_size_multiplies_base() {
        assert_eq!(scaled_window_size(1.0), BASE_WINDOW_SIZE);
        assert_eq!(scaled_window_size(2.0), (600.0, 600.0));
        // 原项目 DEFAULT_SCALE
        assert_eq!(scaled_window_size(0.72), (216.0, 216.0));
    }

    #[test]
    fn scaled_window_size_rejects_non_positive() {
        assert_eq!(scaled_window_size(0.0), BASE_WINDOW_SIZE);
        assert_eq!(scaled_window_size(-1.0), BASE_WINDOW_SIZE);
        assert_eq!(scaled_window_size(f64::NAN), BASE_WINDOW_SIZE);
    }

    #[test]
    fn initial_position_requires_both_coordinates() {
        assert_eq!(initial_position(Some(10.0), Some(20.0)), Some((10, 20)));
        assert_eq!(initial_position(None, Some(20.0)), None);
        assert_eq!(initial_position(Some(10.0), None), None);
        assert_eq!(initial_position(None, None), None);
        // 小数像素截断为整数（与原项目像素坐标语义一致）
        assert_eq!(initial_position(Some(10.9), Some(20.2)), Some((10, 20)));
    }
}
