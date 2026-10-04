//! 动画播放模块：解码链 + 帧调度。
//!
//! 对应原项目 `webm_clip.py` / `decode_fanout.py`（共享解码链、单进程
//! 多窗同角色共享解码器）。Rust 选型（路线图第二阶段落地）：
//! - WebM/VP9 解码：`libwebp`/`ffmpeg-next` 或纯 Rust 的 `symphonia`（音频）+ 视频另行选型
//! - 解码线程与窗口解耦：解码器为独立资源，窗口通过通道（`std::sync::mpsc` / crossbeam）订阅帧
//!
//! 架构红线（继承自原项目）：解码链不得反向依赖 window 模块，窗口钩子只能通过注入接入。

/// 一帧解码结果占位（第二阶段替换为真实帧缓冲）。
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    /// 帧时间戳（毫秒）
    pub timestamp_ms: u64,
    /// 帧序号
    pub index: u32,
}

/// 动画片段占位：负责按需解码并按帧率输出。
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_progresses_with_time() {
        let clip = AnimationClip::new(30.0, 300);
        assert_eq!(clip.frame_at(0).index, 0);
        assert_eq!(clip.frame_at(1000).index, 30);
        assert_eq!(clip.frame_at(10_000).index, 299, "超出时长应停在最后一帧");
    }
}
