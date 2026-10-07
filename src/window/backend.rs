//! 窗口后端抽象：状态机与具体窗口系统的解耦面。
//!
//! 第二阶段接入 `winit` 时提供 `WinitBackend` 实现；本文件只定义接口
//! 与测试用 `MockWindowBackend`，暂不引入任何窗口依赖。

use std::cell::RefCell;
use std::fmt::Write as _;

/// 窗口系统能力面。实现方必须是非阻塞的轻量调用（对应 winit 的
/// `Window` 方法语义），渲染帧仍走 animation 注入的帧订阅。
pub trait WindowBackend {
    /// 显示窗口。
    fn show(&self);
    /// 隐藏窗口。
    fn hide(&self);
    /// 移动窗口到屏幕坐标 (x, y)。
    fn set_position(&self, x: i32, y: i32);
    /// 设置置顶。
    fn set_always_on_top(&self, on: bool);
    /// 请求重绘下一帧。
    fn request_redraw(&self);

    /// 拖拽期间把指针限制在窗口内（issue #36）。
    ///
    /// 语义约定：
    /// - **best-effort / 非保证**：实现方可因平台能力（如 macOS 不支持
    ///   Confined 模式）静默放弃，放弃后行为退化为无跟随保障的现状，
    ///   不视为错误；
    /// - **非阻塞轻量**：单次调用只做模式切换，不含等待/轮询；
    /// - 生命周期由模拟轨驱动：进入拖拽限住、拖拽结束解除，实现方
    ///   不自行推断状态；
    /// - 默认空实现：不支持该能力的后端（含测试 Mock 之外的轻实现）
    ///   无需任何改动。
    fn set_cursor_confined(&self, _on: bool) {}
}

/// 测试用后端：把每个调用记录成 `方法名(参数)` 字符串，供断言调用序列。
#[derive(Debug, Default)]
pub struct MockWindowBackend {
    calls: RefCell<Vec<String>>,
}

impl MockWindowBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// 只读访问已记录的调用序列（供测试断言）。
    pub fn calls(&self) -> Vec<String> {
        self.calls.borrow().clone()
    }

    fn record(&self, entry: &str) {
        self.calls.borrow_mut().push(entry.to_string());
    }
}

impl WindowBackend for MockWindowBackend {
    fn show(&self) {
        self.record("show()");
    }

    fn hide(&self) {
        self.record("hide()");
    }

    fn set_position(&self, x: i32, y: i32) {
        let mut entry = String::new();
        let _ = write!(entry, "set_position({x}, {y})");
        self.record(&entry);
    }

    fn set_always_on_top(&self, on: bool) {
        self.record(if on {
            "set_always_on_top(true)"
        } else {
            "set_always_on_top(false)"
        });
    }

    fn request_redraw(&self) {
        self.record("request_redraw()");
    }

    fn set_cursor_confined(&self, on: bool) {
        self.record(if on {
            "set_cursor_confined(true)"
        } else {
            "set_cursor_confined(false)"
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_call_sequence() {
        let mock = MockWindowBackend::new();
        mock.set_always_on_top(true);
        mock.set_position(10, -3);
        mock.show();
        assert_eq!(
            mock.calls(),
            ["set_always_on_top(true)", "set_position(10, -3)", "show()"]
        );
    }
}
