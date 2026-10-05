//! 窗口状态机：Hidden / Visible / Dragging / Thrown 与合法转移表。
//!
//! 对应原项目 `pet/window.py` 的窗口生命周期（拖起→拖动→抛掷→回落）。
//! 非法转移返回 [`TransitionError`]，调用方自行决定忽略或记录。

use std::fmt;

/// 窗口状态。状态机核心，见模块文档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    /// 隐藏（托盘收纳等）
    Hidden,
    /// 可见且静止
    Visible,
    /// 被拖拽中（鼠标按住移动）
    Dragging,
    /// 抛掷后飞行中，等待物理回落
    Thrown,
}

/// 驱动状态转移的窗口事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    /// 显示窗口（托盘唤出、启动）
    Show,
    /// 隐藏窗口（托盘收纳）
    Hide,
    /// 鼠标按下，准备拖拽
    Press,
    /// 拖拽移动
    Move,
    /// 松开且带初速度 → 抛掷
    ReleaseWithVelocity,
    /// 物理回落着地
    Land,
}

/// 非法状态转移错误：记录来源状态与事件，便于日志定位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransitionError {
    pub from: WindowState,
    pub event: WindowEvent,
}

impl fmt::Display for TransitionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "非法状态转移：{:?} 上收到 {:?}", self.from, self.event)
    }
}

impl std::error::Error for TransitionError {}

/// 合法转移表：唯一的转移规则定义处。
///
/// | 状态 \\ 事件 | Show | Hide | Press | Move | ReleaseWithVelocity | Land |
/// |---|---|---|---|---|---|---|
/// | Hidden      | ✓ Visible | ✗ | ✗ | ✗ | ✗ | ✗ |
/// | Visible     | ✗ | ✓ Hidden | ✓ Dragging | ✗ | ✗ | ✗ |
/// | Dragging    | ✗ | ✗ | ✗ | ✓ Dragging | ✓ Thrown | ✗ |
/// | Thrown      | ✗ | ✗ | ✗ | ✗ | ✗ | ✓ Visible |
pub fn transition(from: WindowState, event: WindowEvent) -> Result<WindowState, TransitionError> {
    let next = match (from, event) {
        (WindowState::Hidden, WindowEvent::Show) => WindowState::Visible,
        (WindowState::Visible, WindowEvent::Hide) => WindowState::Hidden,
        (WindowState::Visible, WindowEvent::Press) => WindowState::Dragging,
        (WindowState::Dragging, WindowEvent::Move) => WindowState::Dragging,
        (WindowState::Dragging, WindowEvent::ReleaseWithVelocity) => WindowState::Thrown,
        (WindowState::Thrown, WindowEvent::Land) => WindowState::Visible,
        _ => return Err(TransitionError { from, event }),
    };
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    use WindowEvent as E;
    use WindowState as S;

    /// 全部合法路径（对齐转移表）。
    #[test]
    fn legal_transitions() {
        let ok = [
            (S::Hidden, E::Show, S::Visible),
            (S::Visible, E::Hide, S::Hidden),
            (S::Visible, E::Press, S::Dragging),
            (S::Dragging, E::Move, S::Dragging),
            (S::Dragging, E::ReleaseWithVelocity, S::Thrown),
            (S::Thrown, E::Land, S::Visible),
        ];
        for (from, event, expect) in ok {
            assert_eq!(transition(from, event), Ok(expect), "{from:?} + {event:?}");
        }
    }

    /// 全部非法路径：4 状态 × 6 事件矩阵减去 6 条合法边 = 18 条。
    #[test]
    fn illegal_transitions() {
        let legal: &[(S, E)] = &[
            (S::Hidden, E::Show),
            (S::Visible, E::Hide),
            (S::Visible, E::Press),
            (S::Dragging, E::Move),
            (S::Dragging, E::ReleaseWithVelocity),
            (S::Thrown, E::Land),
        ];
        let states = [S::Hidden, S::Visible, S::Dragging, S::Thrown];
        let events = [
            E::Show,
            E::Hide,
            E::Press,
            E::Move,
            E::ReleaseWithVelocity,
            E::Land,
        ];
        let mut illegal = 0;
        for &from in &states {
            for &event in &events {
                if legal.contains(&(from, event)) {
                    continue;
                }
                assert_eq!(
                    transition(from, event),
                    Err(TransitionError { from, event })
                );
                illegal += 1;
            }
        }
        assert_eq!(illegal, 4 * 6 - 6);
    }

    /// 错误信息可读且携带上下文。
    #[test]
    fn error_display() {
        let err = transition(S::Hidden, E::Move).unwrap_err();
        assert_eq!(err.to_string(), "非法状态转移：Hidden 上收到 Move");
    }
}
