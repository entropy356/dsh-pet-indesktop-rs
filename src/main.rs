//! dsh-pet-indesktop-rs —— 二进制入口。
//!
//! 第二阶段起步（issue #9）：薄壳启动 winit 事件循环，驱动
//! `request_redraw` → animation 帧订阅。渲染内容本阶段为占位帧
//! （`MockFrameSource` 产出、经 `FrameFanout` 通道送达窗口侧消费），
//! 帧绘制细节（softbuffer 等）另行拆 issue。
//!
//! 模块实现见 `src/lib.rs`；各阶段功能在此逐段接入。

use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use dsh_pet_indesktop_rs::animation::{Frame, MockFrameSource, MonotonicClock, Playback};
use dsh_pet_indesktop_rs::config::{self, SlotConfig};
use dsh_pet_indesktop_rs::window::winit_backend::{self, WinitBackend};
use dsh_pet_indesktop_rs::window::{PetWindow, PetWindowConfig};

/// 事件循环宿主状态：窗口与播放管线的所有权都在这里。
struct PetApp {
    config: SlotConfig,
    playback: Playback<MockFrameSource>,
    clock: MonotonicClock,
    window_rx: Option<std::sync::mpsc::Receiver<Frame>>,
    window: Option<PetWindow>,
    window_id: Option<WindowId>,
    /// winit 窗口句柄副本：PetWindow 内的后端句柄私有（红线 3），
    /// 事件循环侧需要 id / request_redraw 时用这份。
    winit_window: Option<Arc<Window>>,
}

impl PetApp {
    fn new(config: SlotConfig) -> Self {
        // 占位帧源：真实解码链（ffmpeg-next 等）接入后替换。
        Self {
            config,
            playback: Playback::new(MockFrameSource::new(Vec::new())),
            clock: MonotonicClock::new(),
            window_rx: None,
            window: None,
            window_id: None,
            winit_window: None,
        }
    }
}

impl ApplicationHandler for PetApp {
    /// 窗口创建：按 config 构建属性（透明 + 无边框 + 置顶 + 初始尺寸/位置），
    /// 组装 `PetWindow`（注入 `WinitBackend`），订阅 animation 帧通道。
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = winit_backend::window_attributes(&self.config);
        let winit_window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("创建桌宠窗口失败"),
        );
        self.window_id = Some(winit_window.id());

        let backend = WinitBackend::new(Arc::clone(&winit_window));
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

        self.window_rx = Some(self.playback.fanout().subscribe());
        if let Some(w) = &self.winit_window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.window_id != Some(id) {
            return;
        }
        match event {
            // 帧管线占位：消费一帧即视为完成一次绘制（真实绘制另拆 issue）。
            WindowEvent::RedrawRequested => {
                let _ = self.playback.tick(&self.clock);
                if let Some(rx) = &self.window_rx {
                    while let Ok(_frame) = rx.try_recv() {}
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

fn main() {
    println!("dsh-pet-indesktop-rs v{}", env!("CARGO_PKG_VERSION"));
    println!("Rust 重构版桌面宠物 —— 第二阶段：winit 窗口");

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
    /// 骨架自检：版本号元信息非空（编译期元数据存在性）。
    #[test]
    fn skeleton_metadata_present() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
    }
}
