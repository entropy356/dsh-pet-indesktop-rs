//! 桌宠窗口模块：透明无边框、置顶、可拖动、单进程多窗。
//!
//! Rust 选型（路线图第二阶段落地）：
//! - 窗口与事件循环：`winit`（跨平台，支持 Linux Wayland/X11、Windows、macOS）
//! - 透明与置顶：winit `WindowAttributes::with_transparent(true)` + 平台特定扩展
//! - 帧渲染：`softbuffer`（CPU 软渲染起步）或 `wgpu`（后期硬件加速）
//!
//! 对应原项目 `pet/window.py`（约 4600 行，处于"只许瘦不许胖"公约下）。
//! 职责拆分：`state`（状态机）/ `backend`（窗口后端抽象）/ `edge_probe`（边缘探测）。
//!
//! 架构红线（README）：本模块的内部字段仅 `window` 模块自身可访问，
//! 外部只能通过 `PetWindow` 的只读访问器与公开方法交互。

pub mod backend;
pub mod edge_probe;
pub mod state;

use backend::WindowBackend;
use state::{TransitionError, WindowEvent, WindowState};

/// 窗口配置。
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

/// 桌宠窗口：内部字段私有（红线 3），状态经 [`state::transition`] 转移，
/// 窗口系统操作全部委托给注入的 [`WindowBackend`]。
pub struct PetWindow {
    config: PetWindowConfig,
    state: WindowState,
    position: (i32, i32),
    backend: Box<dyn WindowBackend>,
}

impl PetWindow {
    /// 创建窗口（初始 Hidden，由调用方显式 `show` 唤出）。
    pub fn new(config: PetWindowConfig, backend: Box<dyn WindowBackend>) -> Self {
        Self { config, state: WindowState::Hidden, position: (0, 0), backend }
    }

    // ---- 只读访问器（私有面冻结，外部唯一入口） ----

    pub fn config(&self) -> &PetWindowConfig {
        &self.config
    }

    pub fn state(&self) -> WindowState {
        self.state
    }

    pub fn position(&self) -> (i32, i32) {
        self.position
    }

    // ---- 状态转移入口：非法转移返回 Err，且不触碰后端 ----

    /// 显示窗口（Hidden → Visible）。
    pub fn show(&mut self) -> Result<(), TransitionError> {
        self.apply(WindowEvent::Show)
    }

    /// 隐藏窗口（Visible → Hidden）。
    pub fn hide(&mut self) -> Result<(), TransitionError> {
        self.apply(WindowEvent::Hide)
    }

    /// 鼠标按下进入拖拽（Visible → Dragging）。
    pub fn press(&mut self) -> Result<(), TransitionError> {
        self.apply(WindowEvent::Press)
    }

    /// 拖拽移动（Dragging → Dragging），同步窗口位置。
    pub fn drag_to(&mut self, x: i32, y: i32) -> Result<(), TransitionError> {
        let before = self.state;
        self.state = state::transition(before, WindowEvent::Move)?;
        self.position = (x, y);
        self.backend.set_position(x, y);
        self.backend.request_redraw();
        Ok(())
    }

    /// 松开且带初速度，抛掷（Dragging → Thrown）。
    pub fn release_with_velocity(&mut self) -> Result<(), TransitionError> {
        self.apply(WindowEvent::ReleaseWithVelocity)
    }

    /// 抛掷后物理回落着地（Thrown → Visible）。
    pub fn land(&mut self, x: i32, y: i32) -> Result<(), TransitionError> {
        self.state = state::transition(self.state, WindowEvent::Land)?;
        self.position = (x, y);
        self.backend.set_position(x, y);
        self.backend.request_redraw();
        Ok(())
    }

    /// 统一走转移表 + 后端同步的简单事件。
    fn apply(&mut self, event: WindowEvent) -> Result<(), TransitionError> {
        self.state = state::transition(self.state, event)?;
        match event {
            WindowEvent::Show => self.backend.show(),
            WindowEvent::Hide => self.backend.hide(),
            _ => {}
        }
        Ok(())
    }
}

impl std::fmt::Debug for PetWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PetWindow")
            .field("config", &self.config)
            .field("state", &self.state)
            .field("position", &self.position)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use backend::MockWindowBackend;
    use state::TransitionError;

    fn window() -> (PetWindow, std::rc::Rc<MockWindowBackend>) {
        let mock = std::rc::Rc::new(MockWindowBackend::new());
        let backend = Box::new(SharedBackend(std::rc::Rc::clone(&mock)));
        (PetWindow::new(PetWindowConfig::default(), backend), mock)
    }

    /// `Rc` 共享包装：PetWindow 持有 `Box<dyn WindowBackend>`，
    /// 测试同时通过 `Rc` 观察调用记录。
    struct SharedBackend(std::rc::Rc<MockWindowBackend>);

    impl WindowBackend for SharedBackend {
        fn show(&self) {
            self.0.show();
        }
        fn hide(&self) {
            self.0.hide();
        }
        fn set_position(&self, x: i32, y: i32) {
            self.0.set_position(x, y);
        }
        fn set_always_on_top(&self, on: bool) {
            self.0.set_always_on_top(on);
        }
        fn request_redraw(&self) {
            self.0.request_redraw();
        }
    }

    #[test]
    fn full_lifecycle_show_drag_throw_land() {
        let (mut w, mock) = window();
        assert_eq!(w.state(), WindowState::Hidden);

        w.show().unwrap();
        assert_eq!(w.state(), WindowState::Visible);
        assert_eq!(mock.calls(), ["show()"]);

        w.press().unwrap();
        w.drag_to(120, 80).unwrap();
        assert_eq!(w.state(), WindowState::Dragging);
        assert_eq!(w.position(), (120, 80));

        w.release_with_velocity().unwrap();
        assert_eq!(w.state(), WindowState::Thrown);

        w.land(200, 900).unwrap();
        assert_eq!(w.state(), WindowState::Visible);
        assert_eq!(w.position(), (200, 900));
        assert_eq!(
            mock.calls(),
            [
                "show()",
                "set_position(120, 80)",
                "request_redraw()",
                "set_position(200, 900)",
                "request_redraw()",
            ]
        );
    }

    #[test]
    fn hide_show_roundtrip() {
        let (mut w, mock) = window();
        w.show().unwrap();
        w.hide().unwrap();
        assert_eq!(w.state(), WindowState::Hidden);
        w.show().unwrap();
        assert_eq!(w.state(), WindowState::Visible);
        assert_eq!(mock.calls(), ["show()", "hide()", "show()"]);
    }

    /// 非法转移返回 Err 且不触碰后端：Hidden 状态直接拖拽必须被拒。
    #[test]
    fn illegal_event_is_rejected_without_backend_calls() {
        let (mut w, mock) = window();
        assert_eq!(
            w.press(),
            Err(TransitionError { from: WindowState::Hidden, event: WindowEvent::Press })
        );
        assert_eq!(w.state(), WindowState::Hidden);
        assert!(mock.calls().is_empty());
    }

    /// 拖拽中不允许直接隐藏（先松手/落地），表驱动行为的组件级确认。
    #[test]
    fn hide_during_drag_is_rejected() {
        let (mut w, mock) = window();
        w.show().unwrap();
        w.press().unwrap();
        assert!(w.hide().is_err());
        assert_eq!(w.state(), WindowState::Dragging);
        // 后端只收到 show()，hide 未被下发
        assert_eq!(mock.calls(), ["show()"]);
    }
}
