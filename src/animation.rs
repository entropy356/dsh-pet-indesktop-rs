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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Instant;

/// 一帧解码结果（issue #10 契约定稿）。
///
/// 像素数据以 `Arc<[u8]>` 持有：RGBA8、行主序、无 stride padding，
/// 长度恒等于 `width * height * 4`。跨窗共享只递增引用计数，
/// 不做逐订阅者深拷贝（内存峰值最小判据，用户拍板）。
/// 解码侧产出新一帧时整体替换 `Arc`，永不原地改写已共享的 buffer。
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// 帧时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 帧序号
    pub index: u32,
    /// 像素宽（像素）
    pub width: u32,
    /// 像素高（像素）
    pub height: u32,
    /// RGBA8 像素 buffer（行主序，无 stride padding），跨窗零拷贝共享
    pub pixels: Arc<[u8]>,
}

impl Frame {
    /// 构造一帧；`pixels.len() != (width * height * 4)` 时 panic。
    ///
    /// 契约在编译期锚定 buffer 尺寸，构造即校验，坏帧不进入分发链。
    #[must_use]
    pub fn new(timestamp_ms: u64, index: u32, width: u32, height: u32, pixels: Arc<[u8]>) -> Self {
        let expected = width as usize * height as usize * 4;
        assert_eq!(
            pixels.len(),
            expected,
            "Frame 像素 buffer 尺寸须为 width*height*4（RGBA8 无 padding）"
        );
        Self {
            timestamp_ms,
            index,
            width,
            height,
            pixels,
        }
    }

    /// 索引帧占位构造（骨架期 / Mock / 测试）：无像素数据的逻辑帧。
    ///
    /// `pixels` 为空、`width` / `height` 为 0；渲染侧见空 buffer
    /// 即走占位呈现路径（#9 渲染轨按此判定）。
    #[must_use]
    pub fn logical(timestamp_ms: u64, index: u32) -> Self {
        Self {
            timestamp_ms,
            index,
            width: 0,
            height: 0,
            pixels: Arc::from(Vec::new()),
        }
    }

    /// 是否为无像素数据的索引帧（占位路径判定依据）。
    #[must_use]
    pub fn is_logical(&self) -> bool {
        self.pixels.is_empty()
    }
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
        Self {
            start: Instant::now(),
        }
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
        Self {
            now_ms: AtomicU64::new(0),
        }
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
    ///
    /// 换算型帧源只产出索引帧（[`Frame::logical`]），无像素数据；
    /// 真实解码器（#12）按序号查帧后产出带 buffer 的像素帧。
    pub fn frame_at(&self, elapsed_ms: u64) -> Frame {
        let idx = ((elapsed_ms as f64 / 1000.0 * self.fps) as u32)
            .min(self.frame_count.saturating_sub(1));
        Frame::logical(elapsed_ms, idx)
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
            return Frame::logical(clock.now_ms(), 0);
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
        Self {
            source,
            fanout: FrameFanout::new(),
        }
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
/// # 只追最新帧（issue #10 契约定稿）
///
/// 每个订阅通道为 `mpsc::sync_channel(1)`（容量 1），广播端用
/// `try_send`：通道已满（接收侧尚未取走上帧）时**丢弃待投递的帧、
/// 不阻塞解码链**；接收端永远不会积压排队。配合渲染侧
/// 「取帧时排空、只留最新」即可实现只追最新帧语义（取帧节奏归渲染轨 #9）。
///
/// # 零拷贝（issue #10 契约定稿，用户拍板）
///
/// `Frame` 的像素数据为 `Arc<[u8]>`：广播时对每个订阅者只做
/// `Frame` 浅克隆（引用计数 +1），**不做逐订阅者深拷贝整帧**；
/// 解码侧产出新一帧时整体替换 `Arc`，永不原地改写已共享 buffer。
pub struct FrameFanout {
    subscribers: Vec<mpsc::SyncSender<Frame>>,
}

impl FrameFanout {
    pub fn new() -> Self {
        Self {
            subscribers: Vec::new(),
        }
    }

    /// 注册一个订阅窗口，返回其专属接收端。
    ///
    /// 通道容量 1（只追最新帧语义，见类型文档）。
    pub fn subscribe(&mut self) -> mpsc::Receiver<Frame> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.subscribers.push(tx);
        rx
    }

    /// 向所有存活订阅者广播一帧，返回存活订阅数。
    ///
    /// * 接收端已关闭（`Disconnected`）→ 订阅静默摘除；
    /// * 通道已满（`Full`）→ 丢弃投递给该订阅者的本帧，订阅保留
    ///   （窗口还活着，只是没来得及取帧）；
    /// * 帧数据通过 `Arc` 浅克隆共享，无逐订阅者深拷贝。
    pub fn broadcast(&mut self, frame: Frame) -> usize {
        self.subscribers
            .retain(|tx| match tx.try_send(frame.clone()) {
                Ok(()) => true,
                Err(mpsc::TrySendError::Full(_)) => true,
                Err(mpsc::TrySendError::Disconnected(_)) => false,
            });
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
            Frame::logical(0, 0),
            Frame::logical(33, 1),
            Frame::logical(66, 2),
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

        let alive = fanout.broadcast(Frame::logical(10, 3));
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
        let alive = fanout.broadcast(Frame::logical(0, 0));
        assert_eq!(alive, 1, "已关闭窗口的订阅应在广播时摘除");
        assert_eq!(fanout.subscriber_count(), 1);

        let f = rx.recv().expect("存活窗口应正常收帧");
        assert_eq!(f.index, 0);
    }

    #[test]
    fn playback_end_to_end_mock_to_two_windows() {
        let preset: Vec<Frame> = (0..4)
            .map(|i| Frame::logical(i as u64 * 33, i as u32))
            .collect();
        let mut playback = Playback::new(MockFrameSource::new(preset));
        let clock = VirtualClock::new();

        let rx_a = playback.fanout().subscribe();
        let rx_b = playback.fanout().subscribe();

        // 订阅通道容量 1（只追最新帧语义，issue #10 契约定稿）：
        // 每 tick 后即时取帧，逐帧验证双窗送达；消费不及时后续帧被丢弃
        // 而非积压，因此旧语义「先连投再收满 4 帧」不再成立。
        for i in 0..4u64 {
            let (frame, alive) = playback.tick(&clock);
            assert_eq!(alive, 2, "满通道丢帧不改变存活订阅数");
            assert_eq!(frame.index, i as u32 % 4);
            assert_eq!(rx_a.recv().expect("窗口 A 应逐帧送达").index, i as u32 % 4);
            assert_eq!(rx_b.recv().expect("窗口 B 应逐帧送达").index, i as u32 % 4);
            clock.advance_ms(33);
        }

        // 消费后通道已腾空，无残留帧。
        assert!(rx_a.try_recv().is_err());
        assert!(rx_b.try_recv().is_err());
    }

    // ---------- Frame 像素化契约（issue #10） ----------

    #[test]
    fn frame_pixel_buffer_size_is_validated() {
        let pixels = vec![0u8; 2 * 3 * 4]; // 2x3 RGBA8
        let f = Frame::new(0, 0, 2, 3, pixels.into());
        assert_eq!(f.width, 2);
        assert_eq!(f.height, 3);
        assert_eq!(f.pixels.len(), 24);
        assert!(!f.is_logical());
    }

    #[test]
    #[should_panic(expected = "width*height*4")]
    fn frame_rejects_mismatched_pixel_buffer() {
        let pixels = vec![0u8; 10]; // 应为 2*3*4 = 24
        let _ = Frame::new(0, 0, 2, 3, pixels.into());
    }

    #[test]
    fn logical_frame_has_no_pixels() {
        let f = Frame::logical(33, 7);
        assert!(f.is_logical());
        assert_eq!(f.width, 0);
        assert_eq!(f.height, 0);
        assert_eq!(f.timestamp_ms, 33);
        assert_eq!(f.index, 7);
    }

    #[test]
    fn fanout_shares_pixel_buffer_zero_copy() {
        let mut fanout = FrameFanout::new();
        let rx1 = fanout.subscribe();
        let rx2 = fanout.subscribe();

        let pixels = vec![42u8; 4 * 4 * 4];
        let frame = Frame::new(0, 0, 4, 4, pixels.into());
        let sent_ptr = Arc::as_ptr(&frame.pixels);
        assert_eq!(fanout.broadcast(frame), 2);

        let f1 = rx1.recv().expect("窗口 1 应收到帧");
        let f2 = rx2.recv().expect("窗口 2 应收到帧");
        // 零拷贝：两个订阅者拿到的 buffer 与广播帧是同一份堆数据。
        assert!(Arc::ptr_eq(&f1.pixels, &f2.pixels));
        assert_eq!(Arc::as_ptr(&f1.pixels), sent_ptr);
        assert_eq!(f1.pixels[0], 42);
    }

    #[test]
    fn fanout_full_channel_drops_frame_keeps_subscriber() {
        let mut fanout = FrameFanout::new();
        let rx = fanout.subscribe();

        // 不取帧连投两帧：容量 1，第二帧被丢弃（try_send Full → 订阅保留）。
        assert_eq!(fanout.broadcast(Frame::logical(0, 0)), 1);
        assert_eq!(fanout.broadcast(Frame::logical(33, 1)), 1);
        assert_eq!(fanout.subscriber_count(), 1, "满通道不得误摘订阅");

        let f = rx.recv().expect("通道内的首帧应可取出");
        assert_eq!(f.index, 0, "通道内保留的是先投递的帧");
        // 取走后通道腾空，后续广播恢复投递。
        assert_eq!(fanout.broadcast(Frame::logical(66, 2)), 1);
        assert_eq!(rx.recv().expect("腾空后应恢复收帧").index, 2);
    }
}
