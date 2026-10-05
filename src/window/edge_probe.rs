//! 屏幕边缘探测接口占位（对应原项目 edge probe 行为：贴边吸附与抛掷出界判定）。
//!
//! 占位层只定义数据结构与探测函数；后续接入 winit 的监视器信息后，
//! 由真实屏幕矩形驱动，逻辑保持纯函数、可单测。

/// 屏幕边缘（探测目标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenEdge {
    Left,
    Right,
    Top,
    Bottom,
}

/// 屏幕矩形（全局虚拟桌面坐标系，原点左上）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl ScreenRect {
    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }
}

/// 判断点 `(px, py)` 是否落在屏幕某条边的 `threshold` 像素吸附带内。
///
/// 返回最先命中的边缘（左右优先于上下，与原项目探针顺序一致）；
/// 不在任何吸附带内返回 `None`。
pub fn probe(px: i32, py: i32, screen: &ScreenRect, threshold: i32) -> Option<ScreenEdge> {
    let near = |a: i32, b: i32| (a - b).abs() <= threshold;
    if near(px, screen.x) {
        Some(ScreenEdge::Left)
    } else if near(px, screen.right()) {
        Some(ScreenEdge::Right)
    } else if near(py, screen.y) {
        Some(ScreenEdge::Top)
    } else if near(py, screen.bottom()) {
        Some(ScreenEdge::Bottom)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> ScreenRect {
        ScreenRect { x: 0, y: 0, width: 1920, height: 1080 }
    }

    #[test]
    fn detects_all_edges_within_threshold() {
        let s = screen();
        assert_eq!(probe(3, 500, &s, 8), Some(ScreenEdge::Left));
        assert_eq!(probe(1915, 500, &s, 8), Some(ScreenEdge::Right));
        assert_eq!(probe(960, 5, &s, 8), Some(ScreenEdge::Top));
        assert_eq!(probe(960, 1075, &s, 8), Some(ScreenEdge::Bottom));
    }

    #[test]
    fn interior_point_misses() {
        assert_eq!(probe(960, 540, &screen(), 8), None);
    }

    #[test]
    fn outside_screen_beyond_threshold_misses() {
        assert_eq!(probe(-100, 500, &screen(), 8), None);
    }
}
