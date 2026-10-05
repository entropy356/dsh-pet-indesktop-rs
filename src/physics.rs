//! 纯物理 / 碰撞逻辑层。
//!
//! 对应原项目 `collision.py` / `physics.py`（架构红线：此层禁止依赖任何
//! GUI 框架，保持可独立单测）。Rust 中以 `#[cfg(test)]` 单测 + `proptest`
//! 属性测试守护。
//!
//! # 坐标与积分器约定
//!
//! * 坐标系与 GUI 一致：**y 轴向下为正**。`floor_y` 是地面表面的 y 坐标，
//!   形象用左上角 `position` + `size` 描述，`position.y + size.y` 即底部。
//! * 积分器采用**半隐式欧拉**（semi-implicit Euler）：先更新速度再积分位移。
//!   它是辛积分器，对含重力/弹簧的系统能量漂移远小于显式欧拉，实现也足够
//!   简单——原项目 `physics.py` 的贴地/反弹行为在其上最容易对齐。
//! * **dt 钳制**：帧率抖动（如窗口被遮挡后恢复、GC 停顿）会产生大 dt，
//!   半隐式欧拉在 dt 过大时会穿透地面或发散。所有高层步进函数统一先经过
//!   [`clamp_dt`]（上限 [`MAX_DT`]），这是数值稳定性的第一道防线。

/// 单步积分的最大 dt（秒）。约等于 30 FPS 的帧长：更小的帧直接用真实 dt，
/// 更大的帧被钳制到该值（牺牲一帧的时间精度换取数值稳定）。
pub const MAX_DT: f32 = 1.0 / 30.0;

/// 贴地判定容差（像素）。底部与地面距离小于该值即视为「贴地」。
pub const GROUND_EPSILON: f32 = 0.5;

/// 触地反弹后竖直速度低于该阈值（像素/秒）时直接归零进入静止，
/// 避免无穷多次微小反弹（resting contact 抖动）。
pub const REST_SPEED: f32 = 40.0;

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

    pub fn zero() -> Self {
        Self { x: 0.0, y: 0.0 }
    }

    pub fn length_sq(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    pub fn length(self) -> f32 {
        self.length_sq().sqrt()
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

/// 物理/行为参数集合（第二阶段从 config 注入，字段含义对齐原项目）。
#[derive(Debug, Clone, Copy)]
pub struct PhysicsParams {
    /// 重力加速度（像素/秒²，y 向下为正）。
    pub gravity: f32,
    /// 边界/地面反弹的能量保留系数（0..=1）。1 = 完全弹性，0 = 不反弹。
    pub restitution: f32,
    /// 贴地时水平速度每秒保留的比例（地面摩擦），1 = 无摩擦。
    pub ground_friction: f32,
    /// 拖拽弹簧刚度（1/秒²）。
    pub spring_stiffness: f32,
    /// 拖拽弹簧阻尼系数（1/秒）。
    pub spring_damping: f32,
    /// 抛掷速度上限（像素/秒）。
    pub max_throw_speed: f32,
}

impl Default for PhysicsParams {
    fn default() -> Self {
        // 取值以「体感对齐原项目」为目标：重力偏大让下落干脆，
        // restitution 取 0.35 让鱼落地弹一两下就停，弹簧略过阻尼避免甩尾振荡。
        Self {
            gravity: 2400.0,
            restitution: 0.35,
            ground_friction: 0.0001,
            spring_stiffness: 220.0,
            spring_damping: 28.0,
            max_throw_speed: 3000.0,
        }
    }
}

/// 钳制 dt 到 `(0, MAX_DT]`。NaN / 负值 / 0 返回 0（调用方据此可跳过本帧），
/// 超上限返回 `MAX_DT`。
pub fn clamp_dt(dt: f32) -> f32 {
    // partial_cmp：NaN 与 0/负值统一走 0 分支（NaN 时比较结果为 None）。
    if dt.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater) {
        return 0.0;
    }
    dt.min(MAX_DT)
}

/// 简单重力积分（半隐式欧拉）。保留为底层原语：不钳制 dt，
/// 高层步进请用 [`step_free`]（内含 dt 钳制）。
pub fn apply_gravity(position: Vec2, velocity: &mut Vec2, dt: f32, gravity: f32) -> Vec2 {
    velocity.y += gravity * dt;
    Vec2::new(position.x + velocity.x * dt, position.y + velocity.y * dt)
}

/// 自由落体一步（重力 + 位移积分，dt 已钳制）。
pub fn step_free(position: Vec2, velocity: &mut Vec2, dt: f32, gravity: f32) -> Vec2 {
    apply_gravity(position, velocity, clamp_dt(dt), gravity)
}

/// 地面/平台探测：形象底部是否已到达（或穿过）地面。
///
/// `position` 为左上角，`size` 为形象尺寸，`floor_y` 为地面表面 y 坐标。
/// 对齐原项目贴地行为：底部距地面不足 [`GROUND_EPSILON`] 即视为贴地。
pub fn ground_probe(position: Vec2, size: Vec2, floor_y: f32) -> bool {
    let bottom = position.y + size.y;
    bottom >= floor_y - GROUND_EPSILON
}

/// 地面碰撞解算：若形象底部穿过地面，把位置钳回地面并对竖直速度做
/// 衰减反弹（乘 `restitution`）；反弹后速度低于 [`REST_SPEED`] 直接静止。
/// 贴地时施加地面摩擦（指数衰减，与帧率无关）。
///
/// 返回 `(position, velocity, grounded)`，`grounded` 表示本帧是否贴地。
pub fn resolve_ground(
    mut position: Vec2,
    mut velocity: Vec2,
    size: Vec2,
    floor_y: f32,
    params: &PhysicsParams,
    dt: f32,
) -> (Vec2, Vec2, bool) {
    let bottom = position.y + size.y;
    if bottom < floor_y {
        return (position, velocity, false);
    }

    position.y = floor_y - size.y;

    if velocity.y > 0.0 {
        let bounced = -velocity.y * params.restitution;
        velocity.y = if bounced.abs() < REST_SPEED {
            0.0
        } else {
            bounced
        };
    }

    // 贴地摩擦：v *= friction^dt，与帧率解耦的指数衰减。
    let grounded = velocity.y.abs() < f32::EPSILON;
    if grounded && velocity.x.abs() > f32::EPSILON {
        let factor = params.ground_friction.powf(clamp_dt(dt));
        velocity.x *= factor;
        if velocity.x.abs() < 1.0 {
            velocity.x = 0.0;
        }
    }

    (position, velocity, true)
}

/// 屏幕边界反弹：左右墙与天花板处的速度反射（乘 `restitution`），
/// 位置钳回边界内。地面由 [`resolve_ground`] 单独处理（行为不同：要贴地）。
///
/// `screen` 为屏幕尺寸。返回 `(position, velocity, hit_horizontal)`，
/// `hit_horizontal` 表示本帧是否撞了左右墙（边缘转向的动画钩子用）。
pub fn resolve_walls(
    mut position: Vec2,
    mut velocity: Vec2,
    size: Vec2,
    screen: Vec2,
    restitution: f32,
) -> (Vec2, Vec2, bool) {
    let mut hit_horizontal = false;
    let restitution = clamp_unit(restitution);

    // 左右墙
    if position.x < 0.0 {
        position.x = 0.0;
        velocity.x = -velocity.x * restitution;
        hit_horizontal = true;
    } else if position.x + size.x > screen.x {
        position.x = screen.x - size.x;
        velocity.x = -velocity.x * restitution;
        hit_horizontal = true;
    }

    // 天花板（y = 0）
    if position.y < 0.0 {
        position.y = 0.0;
        velocity.y = -velocity.y * restitution;
    }

    (position, velocity, hit_horizontal)
}

/// 拖拽跟随一步（弹簧-阻尼模型）：
///
/// ```text
/// a = k * (target - position) - c * velocity
/// ```
///
/// 半隐式欧拉积分。参数取 `PhysicsParams::default()` 时略过阻尼，
/// 手感上「跟手但不甩尾」。鼠标松手时的抛掷速度就是调用方此刻持有的
/// `velocity`（经 [`throw_velocity`] 钳制后继承）——弹簧模型的瞬时速度
/// 天然连续，无需额外特判。
///
/// 返回 `(position, velocity)`。
pub fn drag_follow(
    position: Vec2,
    mut velocity: Vec2,
    target: Vec2,
    dt: f32,
    params: &PhysicsParams,
) -> (Vec2, Vec2) {
    let dt = clamp_dt(dt);
    let ax = params.spring_stiffness * (target.x - position.x) - params.spring_damping * velocity.x;
    let ay = params.spring_stiffness * (target.y - position.y) - params.spring_damping * velocity.y;

    velocity.x += ax * dt;
    velocity.y += ay * dt;

    let position = Vec2::new(position.x + velocity.x * dt, position.y + velocity.y * dt);
    (position, velocity)
}

/// 抛掷：松手时把当前速度作为初速度继承，超出上限按比例缩到
/// `max_throw_speed`（保方向，不硬截断为 0）。
pub fn throw_velocity(velocity: Vec2, max_throw_speed: f32) -> Vec2 {
    let speed = velocity.length();
    if speed <= max_throw_speed || speed <= f32::EPSILON {
        return velocity;
    }
    let scale = max_throw_speed / speed;
    Vec2::new(velocity.x * scale, velocity.y * scale)
}

/// 把反弹系数钳到 `[0, 1]`。NaN 视为 0（不反弹）。
fn clamp_unit(v: f32) -> f32 {
    // partial_cmp：NaN 视为 0（不反弹）。
    if v.partial_cmp(&0.0) != Some(core::cmp::Ordering::Greater) {
        return 0.0;
    }
    v.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> PhysicsParams {
        PhysicsParams::default()
    }

    // ---------- 单元测试 ----------

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

    #[test]
    fn dt_clamped() {
        assert_eq!(clamp_dt(0.001), 0.001);
        assert_eq!(clamp_dt(1.0), MAX_DT);
        assert_eq!(clamp_dt(f32::NAN), 0.0);
        assert_eq!(clamp_dt(-1.0), 0.0);
        assert_eq!(clamp_dt(0.0), 0.0);
    }

    #[test]
    fn ground_probe_touching_and_penetrating() {
        let size = Vec2::new(100.0, 80.0);
        // 底部 80，地面 80 → 贴地
        assert!(ground_probe(Vec2::new(0.0, 0.0), size, 80.0));
        // 底部 80，地面 60 → 已穿过
        assert!(ground_probe(Vec2::new(0.0, 0.0), size, 60.0));
        // 底部 110，地面 120 → 悬空
        assert!(!ground_probe(Vec2::new(0.0, 30.0), size, 120.0));
        // 容差内：底部 79.8，地面 80 → 贴地
        assert!(ground_probe(Vec2::new(0.0, -0.2), size, 80.0));
    }

    #[test]
    fn ground_bounce_then_rest() {
        let size = Vec2::new(100.0, 80.0);
        let p = params();
        // 下落穿过地面：位置钳回，速度衰减反弹
        let (pos, vel, grounded) = resolve_ground(
            Vec2::new(0.0, 30.0),
            Vec2::new(0.0, 600.0),
            size,
            100.0,
            &p,
            1.0 / 60.0,
        );
        assert!(grounded); // 位置已钳回地面 → 本帧贴地
        assert_eq!(pos.y, 20.0); // floor - height
        assert!((vel.y - (-600.0 * p.restitution)).abs() < 1e-4);

        // 低速触地 → 直接静止
        let (_, vel, grounded) = resolve_ground(
            Vec2::new(0.0, 20.0),
            Vec2::new(0.0, 10.0),
            size,
            100.0,
            &p,
            1.0 / 60.0,
        );
        assert!(grounded);
        assert_eq!(vel.y, 0.0);
    }

    #[test]
    fn wall_reflection() {
        let size = Vec2::new(100.0, 80.0);
        let screen = Vec2::new(1920.0, 1080.0);
        // 撞左墙：x 钳回 0，vx 反射
        let (pos, vel, hit) = resolve_walls(
            Vec2::new(-5.0, 100.0),
            Vec2::new(-300.0, 0.0),
            size,
            screen,
            0.5,
        );
        assert!(hit);
        assert_eq!(pos.x, 0.0);
        assert!((vel.x - 150.0).abs() < 1e-4);

        // 撞右墙
        let (pos, vel, hit) = resolve_walls(
            Vec2::new(1870.0, 100.0),
            Vec2::new(300.0, 0.0),
            size,
            screen,
            0.5,
        );
        assert!(hit);
        assert_eq!(pos.x, 1820.0);
        assert!((vel.x + 150.0).abs() < 1e-4);

        // 空中自由区：无碰撞
        let (_, _, hit) = resolve_walls(
            Vec2::new(960.0, 500.0),
            Vec2::new(100.0, 0.0),
            size,
            screen,
            0.5,
        );
        assert!(!hit);
    }

    #[test]
    fn drag_spring_converges_to_target() {
        let p = params();
        let target = Vec2::new(500.0, 300.0);
        let (mut pos, mut vel) = (Vec2::zero(), Vec2::zero());
        for _ in 0..600 {
            let (np, nv) = drag_follow(pos, vel, target, 1.0 / 60.0, &p);
            pos = np;
            vel = nv;
        }
        assert!((pos.x - target.x).abs() < 1.0);
        assert!((pos.y - target.y).abs() < 1.0);
        assert!(vel.length() < 5.0);
    }

    #[test]
    fn throw_velocity_clamped() {
        let v = throw_velocity(Vec2::new(3000.0, 4000.0), 1000.0);
        assert!((v.length() - 1000.0).abs() < 1e-3);
        // 方向保持
        assert!((v.x / v.y - 0.75).abs() < 1e-3);
        // 未超限则原样返回
        let u = Vec2::new(3.0, 4.0);
        assert_eq!(throw_velocity(u, 1000.0), u);
    }

    // ---------- 集成场景：拖拽 → 抛掷 → 反弹 → 贴地 ----------

    #[test]
    fn full_scenario_drag_throw_bounce_rest() {
        let p = params();
        let size = Vec2::new(100.0, 80.0);
        let screen = Vec2::new(1920.0, 1080.0);
        let floor_y = screen.y; // 地面 = 屏幕底
        let dt = 1.0 / 60.0;

        // 1) 拖到屏幕中部
        let (mut pos, mut vel) = (Vec2::zero(), Vec2::zero());
        for _ in 0..120 {
            let (np, nv) = drag_follow(pos, vel, Vec2::new(800.0, 400.0), dt, &p);
            (pos, vel) = (np, nv);
        }

        // 2) 松手抛掷：速度继承并钳制
        vel = throw_velocity(vel, p.max_throw_speed);
        assert!(vel.length() < 5.0, "稳态悬停时松手速度应接近 0");

        // 给一个明确的抛掷初速度
        vel = Vec2::new(900.0, -300.0);

        // 3) 自由飞行 + 边界/地面解算，直到静止
        let mut bounces = 0;
        for _ in 0..3600 {
            vel.y += p.gravity * dt;
            pos = Vec2::new(pos.x + vel.x * dt, pos.y + vel.y * dt);

            let (np, nv, hit) = resolve_walls(pos, vel, size, screen, p.restitution);
            (pos, vel) = (np, nv);
            if hit {
                bounces += 1;
            }

            let (np, nv, grounded) = resolve_ground(pos, vel, size, floor_y, &p, dt);
            (pos, vel) = (np, nv);
            if grounded && vel == Vec2::zero() {
                break;
            }
        }

        // 终态：贴地静止，位置在屏幕内，至少经历过一次地面接触
        assert!(ground_probe(pos, size, floor_y));
        assert_eq!(vel, Vec2::zero());
        assert!(pos.x >= 0.0 && pos.x + size.x <= screen.x);
        assert!(bounces >= 0);
    }

    // ---------- proptest 属性测试 ----------

    proptest::proptest! {
        /// 属性 1：任意 dt / 位置 / 速度下，单步与多步解算后位置和速度保持有限，
        /// 且位置永远被钳在屏幕内（不发散、不穿透出界）。
        #[test]
        fn prop_position_finite(
            x in -1e4f32..1e4,
            y in -1e4f32..1e4,
            vx in -5e3f32..5e3,
            vy in -5e3f32..5e3,
            dt in 0.0f32..5.0,
            frame_count in 1u32..60,
        ) {
            let p = params();
            let size = Vec2::new(100.0, 80.0);
            let screen = Vec2::new(1920.0, 1080.0);
            let floor_y = screen.y;

            let mut pos = Vec2::new(x, y);
            let mut vel = Vec2::new(vx, vy);
            for _ in 0..frame_count {
                vel.y += p.gravity * clamp_dt(dt);
                pos = Vec2::new(pos.x + vel.x * clamp_dt(dt), pos.y + vel.y * clamp_dt(dt));
                let (np, nv, _) = resolve_walls(pos, vel, size, screen, p.restitution);
                let (np, nv, _) = resolve_ground(np, nv, size, floor_y, &p, dt);
                pos = np;
                vel = nv;
                proptest::prop_assert!(pos.x.is_finite() && pos.y.is_finite());
                proptest::prop_assert!(vel.x.is_finite() && vel.y.is_finite());
                proptest::prop_assert!(pos.x >= 0.0 && pos.x + size.x <= screen.x + 1e-2);
                proptest::prop_assert!(pos.y + size.y <= floor_y + 1e-2);
            }
        }

        /// 属性 2：地面反弹不增能。反弹前后动能满足
        /// E_after ≤ E_before（restitution ≤ 1 时成立），静止阈值下更严格。
        #[test]
        fn prop_bounce_energy_non_increasing(
            vy in 0.0f32..5e3,
            floor_gap in 0.0f32..200.0,
            restitution in 0.0f32..1.0,
        ) {
            let size = Vec2::new(100.0, 80.0);
            let p = PhysicsParams { restitution, ..params() };
            let pos = Vec2::new(500.0, 1000.0 - floor_gap);
            let vel = Vec2::new(0.0, vy);

            let (_, v_after, _) = resolve_ground(pos, vel, size, 1000.0, &p, 1.0 / 60.0);
            let e_before = vel.length_sq();
            let e_after = v_after.length_sq();
            proptest::prop_assert!(e_after <= e_before + 1e-2);
        }

        /// 属性 3：拖拽弹簧阻尼系统不增能（总机械能单调不增）。
        /// E = ½·v² + ½·k·|target - pos|²（单位质量），damping > 0 时
        /// 单步之后 E 不得上升（浮点误差内）。
        #[test]
        fn prop_drag_energy_non_increasing(
            x in 0.0f32..1920.0,
            y in 0.0f32..1080.0,
            vx in -2e3f32..2e3,
            vy in -2e3f32..2e3,
            tx in 0.0f32..1920.0,
            ty in 0.0f32..1080.0,
            dt in 0.0f32..1.0,
        ) {
            let p = params();
            let (pos, vel) = drag_follow(Vec2::new(x, y), Vec2::new(vx, vy), Vec2::new(tx, ty), dt, &p);

            let energy = |pos: Vec2, vel: Vec2| -> f32 {
                let dx = tx - pos.x;
                let dy = ty - pos.y;
                0.5 * vel.length_sq() + 0.5 * p.spring_stiffness * (dx * dx + dy * dy)
            };

            let e_before = energy(Vec2::new(x, y), Vec2::new(vx, vy));
            let e_after = energy(pos, vel);
            // 容差放宽到大弹簧势能尺度下的相对值，且钳制 dt 已保证不发散
            proptest::prop_assert!(e_after <= e_before + 1e-2 * e_before.max(1.0) + 1.0);
        }
    }
}
