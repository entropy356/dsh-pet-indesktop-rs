//! 纯物理 / 碰撞逻辑层。
//!
//! 对应原项目 `collision.py` / `physics.py`（架构红线：此层禁止依赖任何
//! GUI 框架，保持可独立单测）。Rust 中以 `#[cfg(test)]` 单测 + 属性测试守护。

/// 二维向量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// AABB 碰撞盒。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// 与另一 AABB 是否相交。
    pub fn intersects(&self, other: &Aabb) -> bool {
        self.min.x < other.max.x
            && self.max.x > other.min.x
            && self.min.y < other.max.y
            && self.max.y > other.min.y
    }
}

/// 简单重力积分占位（第二阶段对齐原项目 physics.py 的行为）。
pub fn apply_gravity(position: Vec2, velocity: &mut Vec2, dt: f32, gravity: f32) -> Vec2 {
    velocity.y += gravity * dt;
    Vec2::new(position.x + velocity.x * dt, position.y + velocity.y * dt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_intersection() {
        let a = Aabb::new(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let b = Aabb::new(Vec2::new(5.0, 5.0), Vec2::new(15.0, 15.0));
        let c = Aabb::new(Vec2::new(20.0, 20.0), Vec2::new(30.0, 30.0));
        assert!(a.intersects(&b));
        assert!(!a.intersects(&c));
    }

    #[test]
    fn gravity_integrates() {
        let mut v = Vec2::new(0.0, 0.0);
        let p = apply_gravity(Vec2::new(0.0, 0.0), &mut v, 0.5, 10.0);
        // 半隐式欧拉：先更新速度 v.y = 0 + 10*0.5 = 5，再用新速度积分位移 y = 5*0.5 = 2.5
        assert!((v.y - 5.0).abs() < 1e-5);
        assert!((p.y - 2.5).abs() < 1e-5);
    }
}
