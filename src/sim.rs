//! 纯逻辑模拟控制器（issue #11）：把窗口状态机与物理步进接线。
//!
//! 职责：持有 f32 物理真值（位置/速度），把指针输入翻译为窗口状态转移，
//! 并把每步物理解算结果下发给窗口系统。
//!
//! # 与其他模块的边界（对齐 SPEC 红线）
//!
//! * 本模块不依赖任何 GUI 框架；对窗口系统的交互只有两条通道：
//!   - [`PetWindow`] 的**公开方法**（`press` / `drag_to` /
//!     `release_with_velocity` / `land`）——状态机转移的唯一入口，
//!     不触碰 `PetWindow` 私有面（红线 3）；
//!   - [`WindowBackend`] trait 的 `set_position` + `request_redraw`——
//!     Thrown 飞行期间的逐帧位置下发。`PetWindow` 公开 API 不覆盖
//!     Thrown 状态的中间位移，因此经与 `PetWindow` 共享的同一 backend
//!     实例直接下发（生产侧共享注入，测试侧 `Rc<MockWindowBackend>`
//!     共享观察）。
//! * 物理步进全部复用 [`crate::physics`] 现有原语（`drag_follow` /
//!   `step_free` / `resolve_walls` / `resolve_ground` / `throw_velocity`），
//!   本模块不实现新物理。`resolve_ground` 返回的 `grounded` 是**本帧触地**
//!   （contact）语义，与 `ground_probe` 一致——即 issue #11 约定的 `land`
//!   触发条件：首次触地即回 Visible（静止），不等待反弹序列收敛。
//! * 确定性：固定 dt 输入序列下行为完全确定（无随机、无时钟读取），
//!   单测用 `MockWindowBackend` 断言调用序列，属性测试用 proptest。
//!
//! # 步进顺序约定（与 `physics.rs` 集成场景一致）
//!
//! Thrown 期间每步：`step_free`（重力 + 位移）→ `resolve_walls`
//! （左右墙 / 天花板）→ `resolve_ground`（贴地 / 反弹 / 摩擦）。
//! `grounded == true`（本帧触地）即触发 `land(x, y)` 回到 Visible 并把
//! 速度归零（该帧不再做中间帧下发，`land` 自带位置同步与重绘请求）。

use crate::physics::{
    clamp_dt, drag_follow, resolve_ground, resolve_walls, step_free, throw_velocity, PhysicsParams,
    Vec2,
};
use crate::window::backend::WindowBackend;
use crate::window::PetWindow;

/// 指针输入事件（由 GUI 事件循环翻译后注入；坐标为屏幕坐标，y 向下为正）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerEvent {
    /// 鼠标按下（进入拖拽）。
    Press,
    /// 拖拽移动：更新弹簧目标点。
    Move { x: f32, y: f32 },
    /// 松手抛掷：`velocity` 为指针瞬时速度（像素/秒）。
    Release { velocity: Vec2 },
}

/// 模拟场景参数：物理参数 + 形象尺寸 + 屏幕几何。
///
/// `size` 为形象尺寸（像素），`floor_y` 为地面表面 y 坐标，`screen`
/// 为屏幕尺寸（左右墙与天花板边界）。均由 config 注入（第二阶段接线）。
#[derive(Debug, Clone, Copy)]
pub struct SimParams {
    pub physics: PhysicsParams,
    pub size: Vec2,
    pub screen: Vec2,
    pub floor_y: f32,
}

/// 控制器内部相位。与窗口状态机平行维护——本模块只通过 [`PetWindow`]
/// 公开方法的返回值感知转移成败，不读取其私有状态。
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Visible / Hidden 静止。
    Idle,
    /// 拖拽中：`target` 为弹簧目标点（最近一次指针位置）。
    Dragging { target: Vec2 },
    /// 抛掷飞行中。
    Throwing,
}

/// 纯逻辑模拟器：物理真值的唯一持有者。
///
/// 典型接线（主循环）：
///
/// ```text
/// 指针事件 → sim.handle(event, &mut win)
/// 每帧     → sim.step(dt, &mut win, &backend)
/// ```
///
/// 其中 `backend` 与构造 `PetWindow` 的是**同一实例**。
pub struct PetSim {
    params: SimParams,
    position: Vec2,
    velocity: Vec2,
    phase: Phase,
}

impl PetSim {
    /// 创建模拟器。`position` 为形象左上角初始位置（建议取
    /// `win.position()` 换算，保证与窗口起点一致）。
    pub fn new(position: Vec2, params: SimParams) -> Self {
        Self {
            params,
            position,
            velocity: Vec2::zero(),
            phase: Phase::Idle,
        }
    }

    /// 当前物理位置（f32 真值，窗口位置是其取整投影）。
    pub fn position(&self) -> Vec2 {
        self.position
    }

    /// 当前物理速度。
    pub fn velocity(&self) -> Vec2 {
        self.velocity
    }

    /// 是否处于抛掷飞行相位。
    pub fn is_throwing(&self) -> bool {
        self.phase == Phase::Throwing
    }

    /// 是否处于拖拽相位。
    pub fn is_dragging(&self) -> bool {
        matches!(self.phase, Phase::Dragging { .. })
    }

    /// 处理指针事件。非法组合（未按下就 Move/Release、Thrown 中 Press）
    /// 一律忽略——窗口侧转移表同样会拒绝，这里先行拦截避免半途状态。
    pub fn handle(&mut self, event: PointerEvent, win: &mut PetWindow) {
        match (self.phase, event) {
            (Phase::Idle, PointerEvent::Press) => {
                if win.press().is_ok() {
                    self.velocity = Vec2::zero();
                    self.phase = Phase::Dragging {
                        target: self.position,
                    };
                }
            }
            (Phase::Dragging { .. }, PointerEvent::Move { x, y }) => {
                self.phase = Phase::Dragging {
                    target: Vec2::new(x, y),
                };
            }
            (Phase::Dragging { .. }, PointerEvent::Release { velocity })
                if win.release_with_velocity().is_ok() =>
            {
                // 窗口转移成功才进入 Throwing；抛掷速度按上限封顶（保方向）
                self.velocity = throw_velocity(velocity, self.params.physics.max_throw_speed);
                self.phase = Phase::Throwing;
            }
            _ => {}
        }
    }

    /// 步进一次。`dt` 经 [`clamp_dt`] 钳制，非法（NaN / 负 / 0）跳过本帧。
    ///
    /// * Dragging：弹簧跟随一步，新位置经 `win.drag_to` 下发（内部即
    ///   `set_position` + `request_redraw`）。
    /// * Throwing：物理解算一步；贴地时经 `win.land` 回 Visible，未贴地
    ///   时经 backend 下发 `set_position` + `request_redraw`。
    /// * Idle：无操作。
    pub fn step(&mut self, dt: f32, win: &mut PetWindow, backend: &dyn WindowBackend) {
        if clamp_dt(dt) == 0.0 {
            return;
        }
        match self.phase {
            Phase::Idle => {}
            Phase::Dragging { target } => {
                let (pos, vel) = drag_follow(
                    self.position,
                    self.velocity,
                    target,
                    dt,
                    &self.params.physics,
                );
                self.position = pos;
                self.velocity = vel;
                let _ = win.drag_to(self.px(), self.py());
            }
            Phase::Throwing => {
                // 顺序与 physics.rs 集成场景一致：飞行 → 墙 → 地面
                let pos = step_free(
                    self.position,
                    &mut self.velocity,
                    dt,
                    self.params.physics.gravity,
                );
                let (pos, vel, _hit_wall) = resolve_walls(
                    pos,
                    self.velocity,
                    self.params.size,
                    self.params.screen,
                    self.params.physics.restitution,
                );
                let (pos, vel, grounded) = resolve_ground(
                    pos,
                    vel,
                    self.params.size,
                    self.params.floor_y,
                    &self.params.physics,
                    dt,
                );
                self.position = pos;
                self.velocity = vel;

                if grounded {
                    // 首次触地即落地（issue #11 约定：ground_probe 语义触发
                    // land）；Visible = 静止，速度归零。resolve_ground 已把
                    // 位置钳回地面（bottom = floor_y），落点必然合法。
                    self.velocity = Vec2::zero();
                    let _ = win.land(self.px(), self.py());
                    self.phase = Phase::Idle;
                } else {
                    backend.set_position(self.px(), self.py());
                    backend.request_redraw();
                }
            }
        }
    }

    /// 物理位置 → 窗口整数 x（四舍五入）。
    fn px(&self) -> i32 {
        self.position.x.round() as i32
    }

    /// 物理位置 → 窗口整数 y（四舍五入）。
    fn py(&self) -> i32 {
        self.position.y.round() as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::backend::MockWindowBackend;
    use crate::window::state::WindowState;

    /// `Rc` 共享包装（与 window.rs 测试同款思路）：`PetWindow` 持有
    /// `Box<dyn WindowBackend>`，模拟器与测试经同一 `Rc` 观察调用序列。
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

    /// 测试场景：1920×1080 屏幕，形象 100×80，地面 = 屏幕底。
    fn sim_params() -> SimParams {
        SimParams {
            physics: PhysicsParams::default(),
            size: Vec2::new(100.0, 80.0),
            screen: Vec2::new(1920.0, 1080.0),
            floor_y: 1080.0,
        }
    }

    /// 构造（已 show 的窗口 + 初始位置的模拟器 + 共享 mock）。
    fn fixture(start: (f32, f32)) -> (PetSim, PetWindow, std::rc::Rc<MockWindowBackend>) {
        let mock = std::rc::Rc::new(MockWindowBackend::new());
        let backend = Box::new(SharedBackend(std::rc::Rc::clone(&mock)));
        let mut win = PetWindow::new(crate::window::PetWindowConfig::default(), backend);
        win.show().unwrap();
        let sim = PetSim::new(Vec2::new(start.0, start.1), sim_params());
        (sim, win, mock)
    }

    /// `Rc<MockWindowBackend>` → 可步进的 `&dyn WindowBackend`
    /// （与 PetWindow 共享同一实例，调用序列进同一条记录）。
    fn backend(mock: &std::rc::Rc<MockWindowBackend>) -> SharedBackend {
        SharedBackend(std::rc::Rc::clone(mock))
    }

    const DT: f32 = 1.0 / 60.0;

    /// 拖拽跟手：弹簧收敛到指针目标，窗口经 drag_to 同步位置。
    #[test]
    fn drag_follows_pointer() {
        let (mut sim, mut win, mock) = fixture((800.0, 400.0));

        sim.handle(PointerEvent::Press, &mut win);
        assert!(sim.is_dragging());
        assert_eq!(win.state(), WindowState::Dragging);

        // 指针移到 (1000, 300)，步进 60 帧（1 秒）弹簧收敛
        sim.handle(
            PointerEvent::Move {
                x: 1000.0,
                y: 300.0,
            },
            &mut win,
        );
        for _ in 0..60 {
            sim.step(DT, &mut win, &backend(&mock));
        }
        let pos = sim.position();
        assert!((pos.x - 1000.0).abs() < 2.0, "x 未收敛: {pos:?}");
        assert!((pos.y - 300.0).abs() < 2.0, "y 未收敛: {pos:?}");
        // 窗口位置与物理真值取整一致（drag_to 每步下发过）
        assert_eq!(win.position(), (pos.x.round() as i32, pos.y.round() as i32));
    }

    /// 抛出初速度封顶：超限速度按比例缩到 max_throw_speed，方向不变。
    #[test]
    fn throw_velocity_capped_on_release() {
        let (mut sim, mut win, _mock) = fixture((800.0, 400.0));
        sim.handle(PointerEvent::Press, &mut win);

        // 模 5000，上限 3000 → 缩到 (1800, 2400)
        sim.handle(
            PointerEvent::Release {
                velocity: Vec2::new(3000.0, 4000.0),
            },
            &mut win,
        );

        assert!(sim.is_throwing());
        assert_eq!(win.state(), WindowState::Thrown);
        let v = sim.velocity();
        assert!((v.length() - 3000.0).abs() < 1e-3, "速度未封顶: {v:?}");
        assert!((v.x / v.y - 0.75).abs() < 1e-3, "方向未保持: {v:?}");
    }

    /// 回落着地：Thrown 步进最终 land 回 Visible，终态贴地静止、屏内。
    #[test]
    fn thrown_lands_visible_with_legal_position() {
        let (mut sim, mut win, mock) = fixture((800.0, 200.0));
        sim.handle(PointerEvent::Press, &mut win);
        sim.handle(
            PointerEvent::Release {
                velocity: Vec2::new(600.0, -200.0),
            },
            &mut win,
        );

        // 飞行首帧必须经 backend 下发 set_position + request_redraw：
        // vy = -200 + 2400/60 = -160，pos = (810, 197.33)
        let n0 = mock.calls().len();
        sim.step(DT, &mut win, &backend(&mock));
        let calls = mock.calls();
        assert!(calls.len() >= n0 + 2, "Thrown 首帧未下发位置: {calls:?}");
        assert_eq!(calls[calls.len() - 2], "set_position(810, 197)");
        assert_eq!(calls[calls.len() - 1], "request_redraw()");

        // 步进到静止（上限 60 秒）
        let mut landed = false;
        for _ in 0..3600 {
            sim.step(DT, &mut win, &backend(&mock));
            if !sim.is_throwing() {
                landed = true;
                break;
            }
        }
        assert!(landed, "60 秒内未着地");
        assert_eq!(win.state(), WindowState::Visible);

        let p = sim_params();
        let pos = sim.position();
        // 贴地：bottom = floor_y
        assert!(
            (pos.y + p.size.y - p.floor_y).abs() < 1e-2,
            "未贴地: {pos:?}"
        );
        // 屏内
        assert!(pos.x >= 0.0 && pos.x + p.size.x <= p.screen.x);
        // 静止
        assert_eq!(sim.velocity(), Vec2::zero());
        // land 落点与窗口同步
        assert_eq!(win.position(), (pos.x.round() as i32, pos.y.round() as i32));
    }

    /// 左右墙反弹：向左抛出后 x 被钳回边界内、水平速度反向（红线下发）。
    #[test]
    fn wall_bounce_keeps_position_in_bounds() {
        let (mut sim, mut win, mock) = fixture((60.0, 200.0));
        sim.handle(PointerEvent::Press, &mut win);
        sim.handle(
            PointerEvent::Release {
                velocity: Vec2::new(-2000.0, 0.0),
            },
            &mut win,
        );
        // 3 步：前两步 x 由 60 → 26.67 → -6.67（撞左墙钳回 0、vx 反向），
        // 第三步 x = +11.67
        for _ in 0..3 {
            sim.step(DT, &mut win, &backend(&mock));
        }
        let pos = sim.position();
        assert!(pos.x >= 0.0, "穿墙: {pos:?}");
        assert!(sim.velocity().x > 0.0, "未反弹: v={:?}", sim.velocity());
    }

    /// 非法输入忽略：未按下 Move/Release 不产生转移；非法 dt 步进无操作。
    #[test]
    fn illegal_pointer_events_are_ignored() {
        let (mut sim, mut win, mock) = fixture((800.0, 400.0));

        sim.handle(PointerEvent::Move { x: 1.0, y: 1.0 }, &mut win);
        sim.handle(
            PointerEvent::Release {
                velocity: Vec2::new(100.0, 100.0),
            },
            &mut win,
        );
        assert!(!sim.is_dragging() && !sim.is_throwing());
        assert_eq!(sim.velocity(), Vec2::zero());
        // 窗口只收到 fixture 里的 show()，无任何多余调用
        assert_eq!(mock.calls(), ["show()"]);

        // 非法 dt 步进（Idle 相位 + dt 非法）是无操作
        sim.step(0.0, &mut win, &backend(&mock));
        sim.step(f32::NAN, &mut win, &backend(&mock));
        assert_eq!(mock.calls(), ["show()"]);
        assert_eq!(sim.position(), Vec2::new(800.0, 400.0));
    }

    /// Thrown 中 Press 被忽略（窗口转移表 Thrown+Press 非法的组件级确认）。
    #[test]
    fn press_during_throw_is_ignored() {
        let (mut sim, mut win, mock) = fixture((800.0, 200.0));
        sim.handle(PointerEvent::Press, &mut win);
        sim.handle(
            PointerEvent::Release {
                velocity: Vec2::new(0.0, 0.0),
            },
            &mut win,
        );
        assert!(sim.is_throwing());

        sim.handle(PointerEvent::Press, &mut win);
        assert!(sim.is_throwing());
        assert_eq!(win.state(), WindowState::Thrown);
        // 期间窗口/后端无额外调用（除 fixture 的 show()）
        assert_eq!(mock.calls(), ["show()"]);
    }

    // ---- proptest 属性测试 ----

    proptest::proptest! {
        /// 属性：任意合法初位置与抛掷初速度下，模拟最终回到 Visible，
        /// 终态贴地静止、位置全程有限且屏内。固定 dt 序列保证确定性。
        #[test]
        fn prop_throw_always_lands_legally(
            x in 0.0f32..1820.0,
            y in 0.0f32..1000.0,
            vx in -3000.0f32..3000.0,
            vy in -1000.0f32..2000.0,
        ) {
            let params = sim_params();
            let mock = std::rc::Rc::new(MockWindowBackend::new());
            let shared = Box::new(SharedBackend(std::rc::Rc::clone(&mock)));
            let mut win = PetWindow::new(crate::window::PetWindowConfig::default(), shared);
            win.show().unwrap();
            let mut sim = PetSim::new(Vec2::new(x, y), params);
            sim.handle(PointerEvent::Press, &mut win);
            sim.handle(PointerEvent::Release { velocity: Vec2::new(vx, vy) }, &mut win);

            let mut landed = false;
            for _ in 0..7200 {
                sim.step(DT, &mut win, &backend(&mock));
                let p = sim.position();
                proptest::prop_assert!(p.x.is_finite() && p.y.is_finite());
                proptest::prop_assert!(p.x >= 0.0 && p.x + params.size.x <= params.screen.x);
                proptest::prop_assert!(p.y + params.size.y <= params.floor_y + 1e-2);
                if !sim.is_throwing() {
                    landed = true;
                    break;
                }
            }
            proptest::prop_assert!(landed, "120 秒内未着地");
            proptest::prop_assert_eq!(win.state(), WindowState::Visible);
            proptest::prop_assert_eq!(sim.velocity(), Vec2::zero());
            let p = sim.position();
            proptest::prop_assert!((p.y + params.size.y - params.floor_y).abs() < 1e-2);
        }
    }
}
