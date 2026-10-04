//! 动画播放模块：解码链 + 帧调度。
//!
//! 对应原项目 `webm_clip.py` / `decode_fanout.py`（共享解码链、单进程
//! 多窗同角色共享解码器）。
//!
//! # 模块结构（issue #2）
//!
//! * [`Clock`]：时钟抽象。播放时间一律由注入的时钟提供——真实单调钟
//!   [`MonotonicClock`] 用于运行期，测试用虚拟钟 [`VirtualClock`] 手动
//!   推进，彻底替换裸 `elapsed_ms` 参数，让帧调度可确定性测试。
//! * [`FrameSource`]：解码链抽象。第二阶段接入真实解码器
//!   （ffmpeg-next / 平台原生）时实现该 trait 注入，[`MockFrameSource`]
//!   供测试与无素材场景。
//! * [`FrameFanout`]：解码链 → 多窗 fanout 骨架（对应 `decode_fanout.py`）。
//!   每个窗口 `subscribe()` 得到独立 [`mpsc::Receiver`]，解码链广播帧。
//!   窗口钩子只通过通道注入，本模块不反向依赖 window（架构红线）。
//!
//! Rust 选型（路线图第二阶段落地）：
//! - WebM/VP9 解码：`libwebp`/`ffmpeg-next` 或纯 Rust 的 `symphonia`（音频）+ 视频另行选型
//! - 解码线程与窗口解耦：解码器为独立资源，窗口通过通道订阅帧
//!
//! 架构红线（继承自原项目）：解码链不得反向依赖 window 模块，窗口钩子只能通过注入接入。

use std::sync::mpsc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

/// 一帧解码结果占位（第二阶段替换为真实帧缓冲）。
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// 帧时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 帧序号
    pub index: u32,
}

/// 时钟抽象：播放时间线的唯一来源。
///
/// 帧调度只认此 trait，不直接读系统时间——测试注入 [`VirtualClock`]
/// 即可确定性地驱动任意时间线。
pub trait Clock {
    /// 播放时间线当前时刻（毫秒，自播放起点起算）。
    fn now_ms(&self) -> u64;
}

/// 真实时钟：进程内单调钟，自构造时刻起算。
pub struct MonotonicClock {
    start: Instant,
}

impl MonotonicClock {
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

/// 测试用虚拟时钟：手动推进，时间线完全确定。
///
/// 内部用原子量存储，`&self` 即可推进（满足 [`Clock`] 的 `&self` 约束），
/// 也便于将来跨线程驱动。
pub struct VirtualClock {
    now_ms: AtomicU64,
}

impl VirtualClock {
    pub fn new() -> Self {
        Self { now_ms: AtomicU64::new(0) }
    }

    /// 将时间线前移 `delta_ms`。
    pub fn advance_ms(&self, delta_ms: u64) {
        self.now_ms.fetch_add(delta_ms, Ordering::Relaxed);
    }

    /// 直接设置当前时刻（回拨用于边界测试）。
    pub fn set_ms(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::Relaxed);
    }
}

impl Default for VirtualClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::Relaxed)
    }
}

/// 解码链抽象：帧调度的数据来源。
///
/// 第二阶段的真实解码器（ffmpeg-next / 平台原生）实现此 trait 后注入
/// [`Playback`]；测试用 [`MockFrameSource`]。实现方不得持有窗口句柄
/// （架构红线：窗口只能通过 [`FrameFanout`] 通道接收帧）。
pub trait FrameSource {
    /// 由注入时钟驱动，产出当前应显示的一帧。
    fn next_frame(&mut self, clock: &dyn Clock) -> Frame;

    /// 片段总帧数（解码器未知时返回 0）。
    fn frame_count(&self) -> u32;

    /// 片段帧率（fps）。
    fn fps(&self) -> f64;
}

/// 动画片段：按帧率把播放时长换算为帧序号。
///
/// `frame_at` 是纯换算核心（时长 → 帧号，超出时长停在最后一帧）；
/// 作为 [`FrameSource`] 时由注入时钟提供时长。
pub struct AnimationClip {
    pub fps: f64,
    frame_count: u32,
}

impl AnimationClip {
    pub fn new(fps: f64, frame_count: u32) -> Self {
        Self { fps, frame_count }
    }

    /// 计算给定播放时长（毫秒）应显示的帧序号。
    pub fn frame_at(&self, elapsed_ms: u64) -> Frame {
        let idx = ((elapsed_ms as f64 / 1000.0 * self.fps) as u32).min(self.frame_count.saturating_sub(1));
        Frame {
            timestamp_ms: elapsed_ms,
            index: idx,
        }
    }
}

impl FrameSource for AnimationClip {
    fn next_frame(&mut self, clock: &dyn Clock) -> Frame {
        self.frame_at(clock.now_ms())
    }

    fn frame_count(&self) -> u32 {
        self.frame_count
    }

    fn fps(&self) -> f64 {
        self.fps
    }
}

/// 测试替身：按预设帧序列循环产出，并记录被驱动的次数。
///
/// 与 [`AnimationClip`]（fps 换算）不同，Mock 不做任何数学，
/// 用于验证播放循环 / fanout 的管线行为与时间线解耦。
pub struct MockFrameSource {
    frames: Vec<Frame>,
    calls: usize,
}

impl MockFrameSource {
    /// 用预设帧序列构造；空序列会退化为持续产出 0 号帧。
    pub fn new(frames: Vec<Frame>) -> Self {
        Self { frames, calls: 0 }
    }

    /// `next_frame` 被调用的次数（断言驱动节奏用）。
    pub fn call_count(&self) -> usize {
        self.calls
    }
}

impl FrameSource for MockFrameSource {
    fn next_frame(&mut self, clock: &dyn Clock) -> Frame {
        if self.frames.is_empty() {
            self.calls += 1;
            return Frame { timestamp_ms: clock.now_ms(), index: 0 };
        }
        let idx = self.calls % self.frames.len();
        self.calls += 1;
        self.frames[idx].clone()
    }

    fn frame_count(&self) -> u32 {
        self.frames.len() as u32
    }

    fn fps(&self) -> f64 {
        0.0
    }
}

/// 播放循环：帧源 + 时钟 → 逐帧产出，经 [`FrameFanout`] 分发。
///
/// 第二阶段由独立解码线程驱动 `tick`；当前骨架期由调用方（或测试）
/// 显式驱动，保证行为可确定性验证。
pub struct Playback<S: FrameSource> {
    source: S,
    fanout: FrameFanout,
}

impl<S: FrameSource> Playback<S> {
    pub fn new(source: S) -> Self {
        Self { source, fanout: FrameFanout::new() }
    }

    pub fn fanout(&mut self) -> &mut FrameFanout {
        &mut self.fanout
    }

    pub fn source(&mut self) -> &mut S {
        &mut self.source
    }

    /// 推进一帧并广播给所有订阅窗口，返回本帧与存活订阅数。
    pub fn tick(&mut self, clock: &dyn Clock) -> (Frame, usize) {
        let frame = self.source.next_frame(clock);
        let alive = self.fanout.broadcast(frame.clone());
        (frame, alive)
    }
}

/// 解码链 → 多窗 fanout（对应原项目 `decode_fanout.py`）。
///
/// 每个窗口调用 [`subscribe`](FrameFanout::subscribe) 得到独立的
/// `mpsc::Receiver<Frame>`；解码链侧 [`broadcast`](FrameFanout::broadcast)
/// 一帧全员分发。接收端被 drop（窗口关闭）的订阅在下次广播时自动摘除。
///
/// 通道签名占位：第二阶段真实解码线程若需要「只追最新帧」语义，
/// 可换成 `sync_channel(1)` + `try_send`，签名不变。
pub struct FrameFanout {
    subscribers: Vec<mpsc::Sender<Frame>>,
}

impl FrameFanout {
    pub fn new() -> Self {
        Self { subscribers: Vec::new() }
    }

    /// 注册一个订阅窗口，返回其专属接收端。
    pub fn subscribe(&mut self) -> mpsc::Receiver<Frame> {
        let (tx, rx) = mpsc::channel();
        self.subscribers.push(tx);
        rx
    }

    /// 向所有存活订阅者广播一帧，返回存活订阅数。
    /// 发送失败（接收端已关闭）的订阅被静默摘除。
    pub fn broadcast(&mut self, frame: Frame) -> usize {
        self.subscribers.retain(|tx| tx.send(frame.clone()).is_ok());
        self.subscribers.len()
    }

    /// 当前存活订阅数（不触发摘除，仅统计）。
    pub fn subscriber_count(&self) -> usize {
        self.subscribers.len()
    }
}

impl Default for FrameFanout {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------- 帧换算（原 frame_at 测试迁移） ----------

    #[test]
    fn frame_progresses_with_time() {
        let clip = AnimationClip::new(30.0, 300);
        assert_eq!(clip.frame_at(0).index, 0);
        assert_eq!(clip.frame_at(1000).index, 30);
        assert_eq!(clip.frame_at(10_000).index, 299, "超出时长应停在最后一帧");
    }

    #[test]
    fn clip_as_frame_source_driven_by_injected_clock() {
        let mut clip = AnimationClip::new(30.0, 300);
        let clock = VirtualClock::new();

        assert_eq!(clip.next_frame(&clock).index, 0);
        clock.advance_ms(1000);
        assert_eq!(clip.next_frame(&clock).index, 30);
        clock.advance_ms(9_000);
        assert_eq!(clip.next_frame(&clock).index, 299, "超出时长应停在最后一帧");
        assert_eq!(clip.frame_count(), 300);
        assert!((clip.fps() - 30.0).abs() < 1e-9);
    }

    // ---------- Clock 抽象 ----------

    #[test]
    fn virtual_clock_advances_and_sets() {
        let clock = VirtualClock::new();
        assert_eq!(clock.now_ms(), 0);
        clock.advance_ms(250);
        clock.advance_ms(250);
        assert_eq!(clock.now_ms(), 500);
        clock.set_ms(100);
        assert_eq!(clock.now_ms(), 100, "set_ms 支持回拨");
    }

    // ---------- MockFrameSource 驱动完整播放循环 ----------

    #[test]
    fn mock_frame_source_drives_full_playback_loop() {
        let preset = vec![
            Frame { timestamp_ms: 0, index: 0 },
            Frame { timestamp_ms: 33, index: 1 },
            Frame { timestamp_ms: 66, index: 2 },
        ];
        let mut playback = Playback::new(MockFrameSource::new(preset));
        let clock = VirtualClock::new();

        // 直接驱动循环：tick → 帧
        let (f0, _) = playback.tick(&clock);
        assert_eq!(f0.index, 0, "首次调用应取预设帧第 0 帧");
        clock.advance_ms(33);
        let (f1, _) = playback.tick(&clock);
        assert_eq!(f1.index, 1);
        clock.advance_ms(33);
        let (f2, _) = playback.tick(&clock);
        assert_eq!(f2.index, 2);
        clock.advance_ms(33);
        let (f3, _) = playback.tick(&clock);
        assert_eq!(f3.index, 0, "预设帧序列应循环回第 0 帧");
        assert_eq!(playback.source().call_count(), 4);
    }

    // ---------- Fanout：解码链 → 多窗分发 ----------

    #[test]
    fn fanout_broadcasts_to_all_windows() {
        let mut fanout = FrameFanout::new();
        let rx1 = fanout.subscribe();
        let rx2 = fanout.subscribe();
        assert_eq!(fanout.subscriber_count(), 2);

        let alive = fanout.broadcast(Frame { timestamp_ms: 10, index: 3 });
        assert_eq!(alive, 2);

        let f1 = rx1.recv().expect("窗口 1 应收到帧");
        let f2 = rx2.recv().expect("窗口 2 应收到帧");
        assert_eq!(f1, f2);
        assert_eq!(f1.index, 3);
    }

    #[test]
    fn fanout_prunes_closed_windows() {
        let mut fanout = FrameFanout::new();
        let rx = fanout.subscribe();
        let rx_tmp = fanout.subscribe();

        drop(rx_tmp); // 模拟窗口关闭
        let alive = fanout.broadcast(Frame { timestamp_ms: 0, index: 0 });
        assert_eq!(alive, 1, "已关闭窗口的订阅应在广播时摘除");
        assert_eq!(fanout.subscriber_count(), 1);

        let f = rx.recv().expect("存活窗口应正常收帧");
        assert_eq!(f.index, 0);
    }

    #[test]
    fn playback_end_to_end_mock_to_two_windows() {
        let preset: Vec<Frame> = (0..4)
            .map(|i| Frame { timestamp_ms: i as u64 * 33, index: i as u32 })
            .collect();
        let mut playback = Playback::new(MockFrameSource::new(preset));
        let clock = VirtualClock::new();

        let rx_a = playback.fanout().subscribe();
        let rx_b = playback.fanout().subscribe();

        for i in 0..4u64 {
            let (frame, alive) = playback.tick(&clock);
            assert_eq!(alive, 2);
            assert_eq!(frame.index, i as u32 % 4);
            clock.advance_ms(33);
        }

        for rx in [&rx_a, &rx_b] {
            for i in 0..4u64 {
                let f = rx.recv().expect("双窗均应收满 4 帧");
                assert_eq!(f.index, i as u32 % 4);
            }
        }
    }
}
