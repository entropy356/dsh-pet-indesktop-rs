//! dsh-pet-indesktop-rs —— 二进制入口。
//!
//! **三轨集成接线（planner 职责，SPEC §2 禁区例外）**：
//! - 窗口轨（#9/#13）：`WinitBackend` + `PetWindow` + winit 事件循环；
//! - 模拟轨（#11/#23）：`PetSim` 拖拽 / 抛掷物理——指针事件翻译注入，
//!   重绘帧步进；拖拽 / 抛掷期间由 sim 的下发链（`drag_to` /
//!   `set_position` + `request_redraw`）自持重绘循环；
//! - 帧轨（#10/#12）：独立解码线程驱动 `Playback::tick`——PNG 序列帧解码器
//!   （按需解码，素材缺失回退 Mock）；呈现节奏与像素绘制归 #24——Idle 下
//!   不请求重绘属预期，帧动画节奏待 #24 接入。
//!
//! 坐标系约定：sim 与窗口位置统一使用**物理像素屏幕坐标**（与
//! `WinitBackend::set_position` 语义一致）；`CursorMoved` 的窗口相对
//! 坐标经 `outer_position` 换算为屏幕绝对坐标。多显示器按当前显示器
//! 单屏约束（v1 限制，多屏遍历后续拆 issue）。

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use dsh_pet_indesktop_rs::animation::{Frame, FrameSource, Playback};
use dsh_pet_indesktop_rs::config::{self, SlotConfig};
use dsh_pet_indesktop_rs::decode::{frame_source_from_assets, DecodeThread, DEFAULT_ASSETS_DIR};
use dsh_pet_indesktop_rs::physics::{PhysicsParams, Vec2};
use dsh_pet_indesktop_rs::sim::{PetSim, PointerEvent, SimParams};
use dsh_pet_indesktop_rs::window::winit_backend::{self, WinitBackend};
use dsh_pet_indesktop_rs::window::{PetWindow, PetWindowConfig};

/// 解码线程 tick 周期（#10 契约：独立线程驱动 `Playback::tick`）。
/// 30Hz 驱动上限，实际呈现节奏由 fps 与 RedrawRequested 决定（#24）。
const DECODE_TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

/// 事件循环宿主状态：窗口、解码线程与模拟器的所有权都在这里。
struct PetApp {
    config: SlotConfig,
    /// 帧源（真实解码器或 Mock 回退）：`resumed` 装配进解码线程后为 None。
    frame_source: Option<Box<dyn FrameSource + Send>>,
    /// 解码/推进线程（#12）：独立驱动 `Playback::tick`，关闭时优雅退出。
    decode_thread: Option<DecodeThread>,
    window_rx: Option<std::sync::mpsc::Receiver<Frame>>,
    window: Option<PetWindow>,
    window_id: Option<WindowId>,
    /// winit 窗口句柄副本：`PetWindow` 内的后端句柄私有（红线 3），
    /// 事件循环侧需要 id / request_redraw / 几何信息时用这份。
    winit_window: Option<Arc<Window>>,
    /// 模拟控制器（#11/#23）。
    sim: Option<PetSim>,
    /// 与 `PetWindow` 内后端共享同一 `Arc<Window>` 的克隆句柄——
    /// `PetSim::step` 在 Thrown 期间的中间帧下发走它。
    backend: Option<WinitBackend>,
    /// 指针追踪：拖拽目标换算 + 释放速度估计的接线状态。
    pointer: PointerTracker,
    /// 上次 sim 步进时刻（物理 dt 来源）。
    last_step: Option<Instant>,
}

/// 指针样本上限：环形截断，兼顾速度估计窗与内存。
const POINTER_SAMPLES: usize = 32;

/// 释放速度估计的回看时窗：太近噪声大，太远失真。
const RELEASE_VELOCITY_WINDOW: std::time::Duration = std::time::Duration::from_millis(120);

/// 指针追踪器（事件循环侧，非物理真值）。
struct PointerTracker {
    /// 最近样本（屏幕坐标，物理像素）+ 时间戳，旧样本在前。
    samples: std::collections::VecDeque<(Vec2, Instant)>,
    /// 拖拽抓取偏移：形象左上角 − 指针位置（Press 时定格）。
    grab_offset: Option<Vec2>,
}

impl PointerTracker {
    fn new() -> Self {
        Self {
            samples: std::collections::VecDeque::new(),
            grab_offset: None,
        }
    }

    /// 记录一次指针位置（屏幕坐标）。
    fn push(&mut self, pos: Vec2, at: Instant) {
        self.samples.push_back((pos, at));
        while self.samples.len() > POINTER_SAMPLES {
            self.samples.pop_front();
        }
    }

    /// 最近一次指针位置（屏幕坐标）。
    fn last_position(&self) -> Option<Vec2> {
        self.samples.back().map(|&(p, _)| p)
    }

    /// 释放瞬时速度（像素 / 秒）：最后样本与「回看窗之前」最近旧样本差分；
    /// 窗内无旧样本时退化为最早样本差分；样本不足或时距过短（快速点按）
    /// 返回零速度——原地放下。
    fn release_velocity(&self) -> Vec2 {
        let mut iter = self.samples.iter().rev();
        let Some(&(last_pos, last_t)) = iter.next() else {
            return Vec2::zero();
        };
        let cutoff = last_t.checked_sub(RELEASE_VELOCITY_WINDOW);
        let mut old: Option<(Vec2, Instant)> = None;
        if let Some(cut) = cutoff {
            for &(pos, t) in iter {
                if t <= cut {
                    old = Some((pos, t));
                    break;
                }
            }
        }
        // 回看窗内样本过密（全程 <120ms）：退化为最早样本，跨更长时间差分
        let old = old.or_else(|| self.samples.front().copied());
        let Some((old_pos, old_t)) = old else {
            return Vec2::zero();
        };
        let dt = last_t.duration_since(old_t).as_secs_f32();
        if dt < 0.01 {
            return Vec2::zero();
        }
        Vec2::new((last_pos.x - old_pos.x) / dt, (last_pos.y - old_pos.y) / dt)
    }

    /// Press 时定格抓取偏移（无指针样本时偏移为零——极端边路，指针必然
    /// 已在窗口内，正常路径必有样本）。
    fn start_grab(&mut self, pet_pos: Vec2, cursor: Option<Vec2>) {
        self.grab_offset =
            Some(cursor.map_or_else(Vec2::zero, |c| Vec2::new(pet_pos.x - c.x, pet_pos.y - c.y)));
    }

    /// Release 时清除抓取偏移。
    fn end_grab(&mut self) {
        self.grab_offset = None;
    }
}

impl PetApp {
    fn new(config: SlotConfig) -> Self {
        // 帧源（#12）：`assets/<config.character>/` 扫描加载真实解码器；
        // 素材缺失/损坏时回退 Mock 并日志提示（见 decode 模块文档）。
        let frame_source = Some(frame_source_from_assets(
            std::path::Path::new(DEFAULT_ASSETS_DIR),
            &config.character,
        ));
        Self {
            config,
            frame_source,
            decode_thread: None,
            window_rx: None,
            window: None,
            window_id: None,
            winit_window: None,
            sim: None,
            backend: None,
            pointer: PointerTracker::new(),
            last_step: None,
        }
    }

    /// 请求下一帧重绘（事件循环侧统一入口）。
    fn request_redraw(&self) {
        if let Some(w) = &self.winit_window {
            w.request_redraw();
        }
    }
}

impl ApplicationHandler for PetApp {
    /// 窗口创建：按 config 构建属性（透明 + 无边框 + 置顶 + 初始尺寸/位置），
    /// 组装 `PetWindow`（注入 `WinitBackend`）与 `PetSim`，订阅帧通道。
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = winit_backend::window_attributes(&self.config);
        let winit_window = Arc::new(event_loop.create_window(attrs).expect("创建桌宠窗口失败"));
        self.window_id = Some(winit_window.id());

        let backend = WinitBackend::new(Arc::clone(&winit_window));
        // sim 侧共享句柄：WinitBackend 是轻量克隆体（内部 Arc<Window> 同源）
        self.backend = Some(backend.clone());
        let mut pet = PetWindow::new(
            PetWindowConfig {
                always_on_top: self.config.on_top,
                transparent: true,
                scale: self.config.scale as f32,
            },
            Box::new(backend),
        );
        // 启动即唤出（Hidden → Visible），并请求首帧。
        pet.show().expect("初始状态转移 Hidden → Visible 必然合法");
        self.window = Some(pet);
        self.winit_window = Some(winit_window);

        // ---- 模拟轨装配（#11/#23）----
        // 形象尺寸 = 窗口外尺寸（物理像素）；屏幕几何 = 当前（或主）显示器
        // 尺寸，按单屏约束（多屏遍历后续拆 issue）。
        let outer_size = self
            .winit_window
            .as_ref()
            .map(|w| w.outer_size())
            .unwrap_or_default();
        let monitor = self
            .winit_window
            .as_ref()
            .and_then(|w| w.current_monitor().or_else(|| w.primary_monitor()));
        let (screen, floor_y) = match monitor.as_ref() {
            Some(m) => {
                let s = m.size();
                (Vec2::new(s.width as f32, s.height as f32), s.height as f32)
            }
            // 拿不到显示器信息（极少数平台）：保守默认 1080p
            None => (Vec2::new(1920.0, 1080.0), 1080.0),
        };
        let start = self
            .winit_window
            .as_ref()
            .and_then(|w| w.outer_position().ok())
            .map(|p| Vec2::new(p.x as f32, p.y as f32))
            .unwrap_or_else(Vec2::zero);
        self.sim = Some(PetSim::new(
            start,
            SimParams {
                physics: PhysicsParams::default(),
                size: Vec2::new(outer_size.width as f32, outer_size.height as f32),
                screen,
                floor_y,
            },
        ));

        // ---- 帧轨装配（#12）：独立解码线程驱动 Playback::tick（#10 契约
        // 第 4 点），winit 侧仅消费通道——先订阅再移交 playback 所有权。
        let mut playback = Playback::new(
            self.frame_source
                .take()
                .expect("frame_source 仅在 resumed 装配一次"),
        );
        self.window_rx = Some(playback.fanout().subscribe());
        self.decode_thread = Some(DecodeThread::spawn(playback, DECODE_TICK_INTERVAL));
        self.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window_id != Some(id) {
            return;
        }
        // 窗口相对 → 屏幕绝对（物理像素），供指针轨换算
        let outer = self
            .winit_window
            .as_ref()
            .and_then(|w| w.outer_position().ok());
        match event {
            // ---- 指针轨：拖拽目标注入 ----
            WindowEvent::CursorMoved { position, .. } => {
                let cursor = outer.map(|o| {
                    Vec2::new(
                        o.x as f32 + position.x as f32,
                        o.y as f32 + position.y as f32,
                    )
                });
                if let Some(c) = cursor {
                    self.pointer.push(c, Instant::now());
                }
                let offset = self.pointer.grab_offset;
                if let (Some(sim), Some(win)) = (&mut self.sim, &mut self.window) {
                    if sim.is_dragging() {
                        if let (Some(c), Some(off)) = (cursor, offset) {
                            // 弹簧目标 = 指针 + 抓取偏移（维持 Press 时相对关系）
                            sim.handle(
                                PointerEvent::Move {
                                    x: c.x + off.x,
                                    y: c.y + off.y,
                                },
                                win,
                            );
                        }
                    }
                }
                // 驱动下一帧：拖拽弹簧需要逐帧收敛
                self.request_redraw();
            }
            // ---- 指针轨：按下 / 释放（状态转移 + 速度估计） ----
            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            let cursor = self.pointer.last_position();
                            if let (Some(sim), Some(win)) = (&mut self.sim, &mut self.window) {
                                sim.handle(PointerEvent::Press, win);
                                if sim.is_dragging() {
                                    let pet_pos = sim.position();
                                    self.pointer.start_grab(pet_pos, cursor);
                                }
                            }
                        }
                        ElementState::Released => {
                            let velocity = self.pointer.release_velocity();
                            self.pointer.end_grab();
                            if let (Some(sim), Some(win)) = (&mut self.sim, &mut self.window) {
                                sim.handle(PointerEvent::Release { velocity }, win);
                            }
                        }
                    }
                    self.request_redraw();
                }
            }
            // ---- 帧轨消费 + 模拟轨步进（tick 由解码线程驱动，#12）----
            WindowEvent::RedrawRequested => {
                let dt = self
                    .last_step
                    .replace(Instant::now())
                    .map(|t| t.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                // 只追最新帧接收端语义：取帧时排空、只留最新（#10 契约）。
                if let Some(rx) = &self.window_rx {
                    while let Ok(_frame) = rx.try_recv() {}
                }
                if let (Some(sim), Some(win), Some(backend)) =
                    (&mut self.sim, &mut self.window, &self.backend)
                {
                    sim.step(dt, win, backend);
                }
            }
            WindowEvent::CloseRequested => {
                // 解码线程优雅退出（#12 验收）：置停止位 + join。
                if let Some(thread) = self.decode_thread.take() {
                    let _ = thread.stop();
                }
                event_loop.exit();
            }
            _ => {}
        }
    }
}

fn main() {
    println!("dsh-pet-indesktop-rs v{}", env!("CARGO_PKG_VERSION"));
    println!("Rust 重构版桌面宠物 —— 第二阶段：winit 窗口 + 拖拽抛掷");

    // 桌宠 slot 0 配置：存在则读取，否则用默认值（不落盘，落盘属配置持久化 issue）。
    let config_path = std::path::Path::new("config-slot-0.json");
    let slot = config::load_or_default(config_path).unwrap_or_default();

    let event_loop: EventLoop<()> = match EventLoop::with_user_event().build() {
        Ok(loop_) => loop_,
        // 无显示环境（CI/容器）属预期：提示后退出，不 panic。
        Err(err) => {
            eprintln!("无法初始化窗口事件循环（无显示环境？）：{err}");
            eprintln!("窗口行为级验证需在真实桌面环境进行（见 issue #9 验收备注）。");
            std::process::exit(0);
        }
    };

    let mut app = PetApp::new(slot);
    event_loop.set_control_flow(ControlFlow::Wait);
    if let Err(err) = event_loop.run_app(&mut app) {
        eprintln!("事件循环异常退出：{err}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 骨架自检：版本号元信息非空（编译期元数据存在性）。
    #[test]
    fn skeleton_metadata_present() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
    }

    /// 释放速度估计：回看窗选中最旧可用样本，差分方向与量级正确。
    #[test]
    fn release_velocity_estimates_from_lookback_window() {
        let mut tracker = PointerTracker::new();
        let t0 = Instant::now();
        // 样本序列（x 方向匀速 1000 px/s）：0/50/100/150ms
        tracker.push(Vec2::new(100.0, 100.0), t0);
        tracker.push(
            Vec2::new(150.0, 100.0),
            t0 + std::time::Duration::from_millis(50),
        );
        tracker.push(
            Vec2::new(200.0, 100.0),
            t0 + std::time::Duration::from_millis(100),
        );
        tracker.push(
            Vec2::new(250.0, 100.0),
            t0 + std::time::Duration::from_millis(150),
        );

        let v = tracker.release_velocity();
        // 最后样本 @150ms，回看窗 120ms → cutoff 30ms → 选中 0ms 样本
        // v = (250-100)/0.15 = 1000 px/s
        assert!((v.x - 1000.0).abs() < 1.0, "速度估计偏差: {v:?}");
        assert!(v.y.abs() < 1.0, "y 分量应为零: {v:?}");
    }

    /// 样本不足（空 / 单点 / 时距过短）返回零速度——快速点按原地放下。
    #[test]
    fn release_velocity_zero_when_insufficient_samples() {
        let mut tracker = PointerTracker::new();
        assert_eq!(tracker.release_velocity(), Vec2::zero());

        let t0 = Instant::now();
        tracker.push(Vec2::new(10.0, 10.0), t0);
        assert_eq!(tracker.release_velocity(), Vec2::zero());

        // 双样本但时距 5ms（< 10ms 下限）：视为噪声，零速度
        tracker.push(
            Vec2::new(30.0, 10.0),
            t0 + std::time::Duration::from_millis(5),
        );
        assert_eq!(tracker.release_velocity(), Vec2::zero());
    }

    /// 样本环形截断：超过上限后保留最新 POINTER_SAMPLES 条。
    #[test]
    fn pointer_samples_are_ring_capped() {
        let mut tracker = PointerTracker::new();
        let t0 = Instant::now();
        for i in 0..(POINTER_SAMPLES + 10) {
            tracker.push(
                Vec2::new(i as f32, 0.0),
                t0 + std::time::Duration::from_millis(i as u64),
            );
        }
        assert_eq!(tracker.samples.len(), POINTER_SAMPLES);
        // 最早保留样本是第 10 个（i = 10）
        assert_eq!(tracker.samples.front().unwrap().0.x, 10.0);
    }

    /// 抓取偏移：Press 时定格「形象位置 − 指针位置」，无指针样本时为零。
    #[test]
    fn grab_offset_frozen_at_press() {
        let mut tracker = PointerTracker::new();
        tracker.start_grab(Vec2::new(500.0, 400.0), Some(Vec2::new(520.0, 430.0)));
        assert_eq!(tracker.grab_offset, Some(Vec2::new(-20.0, -30.0)));

        tracker.start_grab(Vec2::new(500.0, 400.0), None);
        assert_eq!(tracker.grab_offset, Some(Vec2::zero()));

        tracker.end_grab();
        assert_eq!(tracker.grab_offset, None);
    }
}
