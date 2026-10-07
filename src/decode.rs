//! PNG 序列帧解码器（issue #12）。
//!
//! 纯逻辑模块，零 GUI 依赖（架构红线 R1：解码链不得依赖窗口层；
//! 窗口只能通过 [`FrameFanout`](crate::animation::FrameFanout) 通道接收帧）。
//!
//! # 素材约定（issue #14，本模块的约定来源）
//!
//! ```text
//! assets/<角色>/<行为分类>/<片段>/NNNN.png + manifest.json
//! ```
//!
//! * `manifest.json` 全字段必选（version / fps / frame_count / width / height）；
//! * 帧名 `0000.png` 起零填充 4 位递增，**断号视为损坏**；
//! * manifest 缺失 / 损坏 / 断号 / 尺寸不符 → 该片段整体跳过；
//!   无任何有效片段时 [`frame_source_from_assets`] 回退
//!   [`MockFrameSource`](crate::animation::MockFrameSource) 并日志提示，不 panic。
//!
//! # 设计约束（issue #12 用户决策，优先级：内存 > 磁盘/包体）
//!
//! * **按需解码**：`next_frame` 只解码当前应显示的一帧到 RGBA buffer，
//!   禁止全序列预载 / 预解码（磁盘上的 PNG 即「压缩存储」，内存里常驻
//!   至多一份当前帧）；
//! * **单窗 buffer 复用**：上一帧的 `Arc<[u8]>` 在无外部引用时经
//!   [`Arc::get_mut`] 原地复用，存在共享引用则新分配——守住 #10 契约
//!   「永不原地改写已共享 buffer」；
//! * **跨窗共享**：产出帧经 `Arc<[u8]>` 零拷贝分发（#10 契约锁定项）。
//!
//! # 解码线程（#10 契约第 4 点）
//!
//! [`DecodeThread`] 以独立线程周期驱动
//! [`Playback::tick`](crate::animation::Playback::tick)，winit 侧仅消费
//! `RedrawRequested`；[`DecodeThread::stop`] 置停止位并 join，优雅退出。

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::Deserialize;

use crate::animation::{Clock, Frame, FrameSource, MockFrameSource, MonotonicClock, Playback};

/// 素材根目录（相对工作目录，issue #14 约定）。
pub const DEFAULT_ASSETS_DIR: &str = "assets";

/// manifest schema 版本（issue #14 约定，唯一合法值）。
const MANIFEST_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// manifest 与片段
// ---------------------------------------------------------------------------

/// 片段元信息（由转换脚本经 ffprobe 生成，全字段必选，issue #14）。
#[derive(Debug, Clone, Deserialize)]
pub struct ClipManifest {
    pub version: u32,
    pub fps: f64,
    pub frame_count: u32,
    pub width: u32,
    pub height: u32,
}

impl ClipManifest {
    /// 结构合法性与语义校验（version / fps / 尺寸均为正）。
    fn validate(&self) -> Result<(), AssetError> {
        if self.version != MANIFEST_VERSION {
            return Err(AssetError::Manifest(format!(
                "manifest version={} 非法（期望 {MANIFEST_VERSION}）",
                self.version
            )));
        }
        if !(self.fps.is_finite() && self.fps > 0.0) {
            return Err(AssetError::Manifest(format!("fps={} 非法", self.fps)));
        }
        if self.frame_count == 0 {
            return Err(AssetError::Manifest("frame_count 为 0".into()));
        }
        if self.width == 0 || self.height == 0 {
            return Err(AssetError::Manifest("width/height 必须为正".into()));
        }
        Ok(())
    }
}

/// 一个可播放片段：manifest + 按序号排好的帧文件清单。
#[derive(Debug, Clone)]
pub struct Clip {
    /// 相对 `<角色>` 目录的路径（如 `idle/breathe`）。
    pub name: String,
    /// 行为分类（该目录相对路径的父级，如 `idle`、`events`）。
    pub category: String,
    pub manifest: ClipManifest,
    /// 帧文件路径，按序号升序，长度恒等于 `manifest.frame_count`。
    pub frames: Vec<PathBuf>,
}

/// 素材扫描/解码错误（人工可读，不 panic）。
#[derive(Debug)]
pub enum AssetError {
    Io(io::Error),
    /// manifest 缺失字段、版本不符、字段非法等。
    Manifest(String),
    /// 帧断号、PNG 头不符等结构性损坏。
    Broken(String),
}

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetError::Io(e) => write!(f, "IO 错误：{e}"),
            AssetError::Manifest(msg) => write!(f, "manifest 非法：{msg}"),
            AssetError::Broken(msg) => write!(f, "素材损坏：{msg}"),
        }
    }
}

impl std::error::Error for AssetError {}

impl From<io::Error> for AssetError {
    fn from(e: io::Error) -> Self {
        AssetError::Io(e)
    }
}

impl From<png::DecodingError> for AssetError {
    fn from(e: png::DecodingError) -> Self {
        AssetError::Broken(format!("PNG 解码失败：{e}"))
    }
}

// ---------------------------------------------------------------------------
// 素材库扫描
// ---------------------------------------------------------------------------

/// `assets/<character>/` 扫描结果：全部有效片段（按名称升序）。
#[derive(Debug, Clone)]
pub struct AssetLibrary {
    pub character: String,
    pub clips: Vec<Clip>,
}

impl AssetLibrary {
    /// 递归扫描 `base/<character>/` 下含 `manifest.json` 的目录
    /// （约定：**含 manifest.json 的目录 = 一个可播放片段**）。
    ///
    /// 单个片段 manifest 缺失/损坏/断号 → 跳过该片段（日志提示），
    /// 不影响其余片段；角色目录不存在 → 返回 `Err`。
    pub fn scan(base: &Path, character: &str) -> Result<Self, AssetError> {
        let root = base.join(character);
        if !root.is_dir() {
            return Err(AssetError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("角色素材目录不存在：{}", root.display()),
            )));
        }
        let mut clips = Vec::new();
        scan_dir(&root, &root, &mut clips);
        clips.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self {
            character: character.to_string(),
            clips,
        })
    }

    /// 默认片段：优先行为分类 `idle`（片段名 `idle` 或 `idle/` 前缀），
    /// 否则取名称最小的片段。无有效片段返回 `None`。
    pub fn default_clip(&self) -> Option<&Clip> {
        self.clips
            .iter()
            .find(|c| c.category == "idle")
            .or_else(|| self.clips.first())
    }
}

/// 深度优先遍历：含 manifest.json 的目录按片段收集，其余继续下钻。
fn scan_dir(root: &Path, dir: &Path, clips: &mut Vec<Clip>) {
    let manifest_path = dir.join("manifest.json");
    if manifest_path.is_file() {
        match load_clip(root, dir, &manifest_path) {
            Ok(clip) => clips.push(clip),
            Err(e) => eprintln!(
                "[decode] 跳过无效片段 {}（{}）",
                dir.strip_prefix(root).unwrap_or(dir).to_string_lossy(),
                e
            ),
        }
        return; // 片段目录不再下钻（嵌套片段不在约定内）
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            scan_dir(root, &entry.path(), clips);
        }
    }
}

/// 加载并校验单个片段：manifest 解析 + 校验 + 帧文件清单断号检查。
fn load_clip(root: &Path, dir: &Path, manifest_path: &Path) -> Result<Clip, AssetError> {
    let raw = fs::read_to_string(manifest_path)?;
    let manifest: ClipManifest = serde_json::from_str(&raw)
        .map_err(|e| AssetError::Manifest(format!("{} 解析失败：{e}", manifest_path.display())))?;
    manifest.validate()?;

    let name = dir
        .strip_prefix(root)
        .unwrap_or(dir)
        .to_string_lossy()
        .replace('\\', "/");
    // 行为分类 = 片段目录相对路径的父级（可多层，如 events/balance）；
    // 顶层片段目录无分类层，记空串，default_clip 以 first 兜底。
    let category = match name.rsplit_once('/') {
        Some((parent, _)) => parent.to_string(),
        None => String::new(),
    };

    let mut frames = Vec::with_capacity(manifest.frame_count as usize);
    for i in 0..manifest.frame_count {
        let frame_path = dir.join(format!("{i:04}.png"));
        if !frame_path.is_file() {
            return Err(AssetError::Broken(format!(
                "片段「{name}」帧 {i:04}.png 缺失（断号视为损坏）"
            )));
        }
        frames.push(frame_path);
    }

    // 首帧 PNG 头校验：尺寸须与 manifest 一致（后续帧在解码时逐帧校验）。
    let (w, h) = png_header_size(&frames[0])?;
    if (w, h) != (manifest.width, manifest.height) {
        return Err(AssetError::Broken(format!(
            "片段「{name}」首帧 {w}x{h} 与 manifest {}x{} 不符",
            manifest.width, manifest.height
        )));
    }

    Ok(Clip {
        name,
        category,
        manifest,
        frames,
    })
}

/// 读取 PNG 头（IHDR），返回 (宽, 高)。只读头部字节，不解码像素。
fn png_header_size(path: &Path) -> Result<(u32, u32), AssetError> {
    let file = io::BufReader::new(fs::File::open(path)?);
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::IDENTITY);
    let reader = decoder.read_info()?;
    Ok((reader.info().width, reader.info().height))
}

// ---------------------------------------------------------------------------
// FrameSource 实现：按需解码 + buffer 复用
// ---------------------------------------------------------------------------

/// PNG 序列帧解码器（[`FrameSource`] 实现）。
///
/// 每次 [`next_frame`](FrameSource::next_frame) 按时钟换算目标序号，
/// 现场打开对应 PNG 解码到 RGBA8（**按需解码，无预载**）。
pub struct PngSequenceSource {
    clip: Clip,
    /// 上一帧像素 buffer：无外部引用时原地复用（单窗 buffer 复用约束）。
    last_pixels: Option<Arc<[u8]>>,
    /// 已执行的真实解码次数（诊断 / 测试「按需解码」约束用）。
    decode_count: u64,
}

impl PngSequenceSource {
    pub fn new(clip: Clip) -> Self {
        Self {
            clip,
            last_pixels: None,
            decode_count: 0,
        }
    }

    /// 已执行的真实解码次数。
    pub fn decode_count(&self) -> u64 {
        self.decode_count
    }

    /// 产出一帧的像素 buffer：
    /// * 上一帧 buffer 仍被共享（`strong_count > 1`）→ 新分配，
    ///   保证 #10 契约「永不原地改写已共享 buffer」；
    /// * 上一帧 buffer 已独占 → `Arc::get_mut` 原地复用（零分配）。
    fn slot_pixels(
        &mut self,
        width: u32,
        height: u32,
        path: &Path,
    ) -> Result<Arc<[u8]>, AssetError> {
        let expected = width as usize * height as usize * 4;
        let reusable = self
            .last_pixels
            .as_ref()
            .is_some_and(|arc| Arc::strong_count(arc) == 1 && arc.len() == expected);
        if reusable {
            let arc = self.last_pixels.as_mut().expect("reusable 已判定存在");
            let buf = Arc::get_mut(arc).expect("strong_count == 1 时 get_mut 必成功");
            decode_rgba_into(path, width, height, buf)?;
            self.decode_count += 1;
            Ok(Arc::clone(arc))
        } else {
            let mut buf = vec![0u8; expected];
            decode_rgba_into(path, width, height, &mut buf)?;
            self.decode_count += 1;
            let arc: Arc<[u8]> = buf.into();
            self.last_pixels = Some(Arc::clone(&arc));
            Ok(arc)
        }
    }
}

impl FrameSource for PngSequenceSource {
    fn next_frame(&mut self, clock: &dyn Clock) -> Frame {
        let now = clock.now_ms();
        let (fps, frame_count, width, height) = {
            let m = &self.clip.manifest;
            (m.fps, m.frame_count, m.width, m.height)
        };
        let idx = ((now as f64 / 1000.0 * fps) as u64 % frame_count as u64) as u32;
        let path = self.clip.frames[idx as usize].clone();
        match self.slot_pixels(width, height, &path) {
            Ok(pixels) => Frame::new(now, idx, width, height, pixels),
            // 单帧解码失败：占位帧兜底 + 日志，不 panic、不回退整个片段
            //（其余帧大概率完好，片段级回退交由上层按需决策）。
            Err(e) => {
                eprintln!(
                    "[decode] 片段「{}」帧 {idx:04} 解码失败（{e}），本帧走占位",
                    self.clip.name
                );
                Frame::logical(now, idx)
            }
        }
    }

    fn frame_count(&self) -> u32 {
        self.clip.manifest.frame_count
    }

    fn fps(&self) -> f64 {
        self.clip.manifest.fps
    }
}

/// 把 `path` 的 PNG 解码为 RGBA8 写入 `out`（长度须恰为 `width*height*4`）。
///
/// 变换组合 `EXPAND | ALPHA | STRIP_16`：调色板展开、补齐 alpha、
/// 16-bit 降 8-bit；输出若仍非 RGBA8（如灰度图）视为与运行时契约
/// 不符的素材，报损坏。
fn decode_rgba_into(
    path: &Path,
    width: u32,
    height: u32,
    out: &mut [u8],
) -> Result<(), AssetError> {
    let file = io::BufReader::new(fs::File::open(path)?);
    let mut decoder = png::Decoder::new(file);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info()?;
    {
        let info = reader.info();
        if (info.width, info.height) != (width, height) {
            return Err(AssetError::Broken(format!(
                "{} 实际尺寸 {}x{} 与 manifest {}x{} 不符",
                path.display(),
                info.width,
                info.height,
                width,
                height
            )));
        }
    }
    let (color, depth) = reader.output_color_type();
    if !matches!(color, png::ColorType::Rgba) || depth != png::BitDepth::Eight {
        return Err(AssetError::Broken(format!(
            "{} 解码输出 {color:?}/{depth:?} 非 RGBA8（素材须为 RGB/RGBA 8-bit）",
            path.display()
        )));
    }
    let expect = reader.output_buffer_size().unwrap_or_default();
    if out.len() != expect {
        return Err(AssetError::Broken(format!(
            "{} 输出 buffer 尺寸 {} 与期望 {} 不符",
            path.display(),
            expect,
            out.len()
        )));
    }
    reader.next_frame(out)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// config 加载路径 + 回退
// ---------------------------------------------------------------------------

/// 按 #14 目录约定 + config 作用域构建帧源（issue #12 范围 3）。
///
/// 扫描 `base/<character>/`；无任何有效片段时回退
/// [`MockFrameSource`]（空预设 = 索引帧占位）并日志提示。
/// 行为分类/片段切换、播放编排归后续 issue（本函数只选默认片段）。
pub fn frame_source_from_assets(base: &Path, character: &str) -> Box<dyn FrameSource + Send> {
    match AssetLibrary::scan(base, character) {
        Ok(lib) => match lib.default_clip() {
            Some(clip) => {
                eprintln!(
                    "[decode] 已加载角色「{character}」片段「{}」（{} 帧，{:.1} fps）",
                    clip.name, clip.manifest.frame_count, clip.manifest.fps
                );
                Box::new(PngSequenceSource::new(clip.clone()))
            }
            None => {
                eprintln!("[decode] 角色「{character}」目录下无有效片段，回退 MockFrameSource");
                Box::new(MockFrameSource::new(Vec::new()))
            }
        },
        Err(e) => {
            eprintln!("[decode] 素材扫描失败（{e}），回退 MockFrameSource");
            Box::new(MockFrameSource::new(Vec::new()))
        }
    }
}

// ---------------------------------------------------------------------------
// 解码线程
// ---------------------------------------------------------------------------

/// 独立解码/推进线程（#10 契约第 4 点）。
///
/// 持有 [`Playback`] 周期驱动 `tick`；窗口侧通过通道消费帧、
/// 仅响应 `RedrawRequested`，不参与推进。[`DecodeThread::stop`]
/// 置停止位并 join；`Drop` 兜底置位（不 join，避免阻塞析构路径）。
pub struct DecodeThread {
    handle: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl DecodeThread {
    /// 以 `tick_interval` 为节奏 spawn 推进线程。
    ///
    /// 注意：调用方须先完成
    /// [`subscribe`](crate::animation::Playback::fanout)（订阅在 spawn 前），
    /// 否则窗口拿不到帧。
    pub fn spawn<S>(mut playback: Playback<S>, tick_interval: Duration) -> Self
    where
        S: FrameSource + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            let clock = MonotonicClock::new();
            while !flag.load(Ordering::Relaxed) {
                let _ = playback.tick(&clock);
                thread::sleep(tick_interval);
            }
        });
        Self {
            handle: Some(handle),
            stop,
        }
    }

    /// 置停止位并等待线程退出（优雅退出）。
    pub fn stop(mut self) -> std::thread::Result<()> {
        self.stop.store(true, Ordering::Relaxed);
        match self.handle.take() {
            Some(handle) => handle.join(),
            None => Ok(()),
        }
    }

    /// 线程是否仍在运行（仅诊断用）。
    pub fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for DecodeThread {
    fn drop(&mut self) {
        // 兜底置位；不 join——正常退出路径走 stop()。
        self.stop.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// 测试（素材在临时目录现场生成，不入库——issue #12 素材策略）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation::VirtualClock;

    /// 测试根目录：临时目录 + 唯一后缀。
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dsh-decode-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 写一张 RGBA8 PNG。
    fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
    }

    /// 写一张 RGB（无 alpha）PNG。
    fn write_png_rgb(path: &Path, width: u32, height: u32, rgb: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(path).unwrap();
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(rgb).unwrap();
    }

    fn write_manifest(dir: &Path, fps: f64, frame_count: u32, width: u32, height: u32) {
        let json = format!(
            r#"{{ "version": 1, "fps": {fps}, "frame_count": {frame_count}, "width": {width}, "height": {height} }}"#
        );
        fs::write(dir.join("manifest.json"), json).unwrap();
    }

    /// 生成完整片段：`<root>/<character>/<category>/<clip>/`，每帧像素可辨识。
    fn make_clip(
        root: &Path,
        character: &str,
        category: &str,
        clip: &str,
        n: u32,
        w: u32,
        h: u32,
    ) -> PathBuf {
        let dir = root.join(character).join(category).join(clip);
        for i in 0..n {
            let mut px = vec![0u8; (w * h * 4) as usize];
            for (j, b) in px.iter_mut().enumerate() {
                *b = ((i as usize + j) % 256) as u8;
            }
            write_png(&dir.join(format!("{i:04}.png")), w, h, &px);
        }
        write_manifest(&dir, 24.0, n, w, h);
        dir
    }

    // ---------- 扫描与校验 ----------

    #[test]
    fn scan_finds_clips_with_category_and_sorted_frames() {
        let root = temp_root("scan-ok");
        make_clip(&root, "shenshen", "idle", "breathe", 4, 2, 2);
        make_clip(&root, "shenshen", "events/balance", "wave", 2, 2, 2);

        let lib = AssetLibrary::scan(&root, "shenshen").expect("扫描应成功");
        assert_eq!(lib.clips.len(), 2);
        assert_eq!(lib.clips[0].name, "events/balance/wave");
        assert_eq!(lib.clips[0].category, "events/balance");
        assert_eq!(lib.clips[0].frames.len(), 2);
        assert_eq!(lib.clips[1].name, "idle/breathe");
        assert_eq!(lib.clips[1].category, "idle");
        // default_clip 优先 idle 分类
        assert_eq!(lib.default_clip().unwrap().name, "idle/breathe");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_skips_gap_broken_clip_and_keeps_valid_one() {
        let root = temp_root("scan-gap");
        make_clip(&root, "shenshen", "idle", "breathe", 4, 2, 2);
        // 第二个片段删掉一帧 → 断号 → 整体跳过
        let broken = make_clip(&root, "shenshen", "click", "hop", 3, 2, 2);
        fs::remove_file(broken.join("0001.png")).unwrap();

        let lib = AssetLibrary::scan(&root, "shenshen").expect("扫描本身应成功");
        assert_eq!(lib.clips.len(), 1, "断号片段应被跳过");
        assert_eq!(lib.clips[0].name, "idle/breathe");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_skips_manifest_with_missing_field_or_bad_version() {
        let root = temp_root("scan-manifest");
        make_clip(&root, "shenshen", "idle", "breathe", 2, 2, 2);
        // 缺字段
        let dir = root.join("shenshen/click/hop");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..2u32 {
            write_png(&dir.join(format!("{i:04}.png")), 2, 2, &[1; 16]);
        }
        fs::write(dir.join("manifest.json"), r#"{ "version": 1, "fps": 24 }"#).unwrap();
        // 版本非法
        let dir2 = root.join("shenshen/click/jump");
        fs::create_dir_all(&dir2).unwrap();
        for i in 0..2u32 {
            write_png(&dir2.join(format!("{i:04}.png")), 2, 2, &[1; 16]);
        }
        write_manifest(&dir2, 24.0, 2, 2, 2);
        let bad = r#"{ "version": 99, "fps": 24, "frame_count": 2, "width": 2, "height": 2 }"#;
        fs::write(dir2.join("manifest.json"), bad).unwrap();

        let lib = AssetLibrary::scan(&root, "shenshen").expect("扫描本身应成功");
        assert_eq!(lib.clips.len(), 1, "非法 manifest 片段应被跳过");
        assert_eq!(lib.clips[0].name, "idle/breathe");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn scan_errors_on_missing_character_dir() {
        let root = temp_root("scan-missing");
        assert!(matches!(
            AssetLibrary::scan(&root, "nobody"),
            Err(AssetError::Io(_))
        ));
        fs::remove_dir_all(&root).ok();
    }

    // ---------- 解码：按需 / RGBA / 复用 ----------

    #[test]
    fn decodes_rgba_on_demand_without_preload() {
        let root = temp_root("decode-demand");
        make_clip(&root, "shenshen", "idle", "breathe", 4, 2, 2);
        let lib = AssetLibrary::scan(&root, "shenshen").unwrap();
        let mut src = PngSequenceSource::new(lib.default_clip().unwrap().clone());

        let clock = VirtualClock::new();
        // 未 tick 时零解码（禁止预载）
        assert_eq!(src.decode_count(), 0);

        let f0 = src.next_frame(&clock);
        assert!(!f0.is_logical());
        assert_eq!((f0.width, f0.height), (2, 2));
        assert_eq!(f0.pixels.len(), 16);
        assert_eq!(f0.index, 0);
        assert_eq!(src.decode_count(), 1, "一次 tick 恰一次解码");

        clock.advance_ms(1000); // 24fps → 第 24 帧 % 4 = 第 0 帧? 1000ms*24/1000=24 → idx 0
        let f1 = src.next_frame(&clock);
        assert_eq!(f1.index, 0);
        assert_eq!(src.decode_count(), 2, "每个显示帧各解码一次");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn decodes_pixel_content_across_frame_indices() {
        let root = temp_root("decode-content");
        make_clip(&root, "shenshen", "idle", "breathe", 4, 2, 2);
        let lib = AssetLibrary::scan(&root, "shenshen").unwrap();
        let mut src = PngSequenceSource::new(lib.default_clip().unwrap().clone());

        let clock = VirtualClock::new();
        // 41.7ms → idx 1：像素应与磁盘上 0001.png 一致
        clock.advance_ms(41);
        let f = src.next_frame(&clock);
        assert_eq!(f.index, 0, "41ms*24fps=0.984 → idx 0");

        clock.advance_ms(42); // 累计 83ms*24 = 1.992 → idx 1
        let f = src.next_frame(&clock);
        assert_eq!(f.index, 1);
        let expected = {
            let mut buf = Vec::new();
            let file = io::BufReader::new(fs::File::open(&lib.clips[0].frames[1]).unwrap());
            let mut dec = png::Decoder::new(file);
            dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
            let mut r = dec.read_info().unwrap();
            buf.resize(r.output_buffer_size().unwrap_or_default(), 0);
            r.next_frame(&mut buf).unwrap();
            buf
        };
        assert_eq!(f.pixels.as_ref(), expected.as_slice());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rgb_png_gets_opaque_alpha() {
        let root = temp_root("decode-rgb");
        let dir = root.join("shenshen/idle/flat");
        fs::create_dir_all(&dir).unwrap();
        write_png_rgb(&dir.join("0000.png"), 2, 1, &[10, 20, 30, 40, 50, 60]);
        write_manifest(&dir, 24.0, 1, 2, 1);

        let lib = AssetLibrary::scan(&root, "shenshen").unwrap();
        let mut src = PngSequenceSource::new(lib.default_clip().unwrap().clone());
        let f = src.next_frame(&VirtualClock::new());
        assert!(!f.is_logical());
        assert_eq!(f.pixels.as_ref(), &[10, 20, 30, 255, 40, 50, 60, 255]);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn buffer_reuses_only_when_exclusive() {
        let root = temp_root("decode-reuse");
        make_clip(&root, "shenshen", "idle", "breathe", 4, 2, 2);
        let lib = AssetLibrary::scan(&root, "shenshen").unwrap();
        let mut src = PngSequenceSource::new(lib.default_clip().unwrap().clone());

        let clock = VirtualClock::new();
        clock.advance_ms(41);
        let f0 = src.next_frame(&clock); // 分配 A
        let ptr_a = Arc::as_ptr(&f0.pixels);

        let f1 = src.next_frame(&clock); // f0 仍持有 A → 必须新分配 B
        let ptr_b = Arc::as_ptr(&f1.pixels);
        assert_ne!(ptr_a, ptr_b, "共享中的 buffer 不得被原地改写复用");

        drop(f0);
        drop(f1); // B 独占
        let f2 = src.next_frame(&clock); // 复用 B
        assert_eq!(
            Arc::as_ptr(&f2.pixels),
            ptr_b,
            "无外部引用时应原地复用 buffer"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn single_corrupt_frame_falls_back_to_logical_without_panic() {
        let root = temp_root("decode-corrupt");
        make_clip(&root, "shenshen", "idle", "breathe", 2, 2, 2);
        let lib = AssetLibrary::scan(&root, "shenshen").unwrap();
        // 尺寸与 manifest 不符的坏帧
        write_png(&lib.clips[0].frames[1], 4, 4, &[9; 64]);
        let mut src = PngSequenceSource::new(lib.clips[0].clone());

        let clock = VirtualClock::new();
        clock.advance_ms(83); // idx 1 → 坏帧 → 占位
        let f = src.next_frame(&clock);
        assert!(f.is_logical(), "坏帧应走占位兜底");
        assert_eq!(f.index, 1);

        clock.advance_ms(84); // 累计 167ms*24fps = 4.008 → idx 0 → 好帧不受影响
        let f = src.next_frame(&clock);
        assert!(!f.is_logical());
        fs::remove_dir_all(&root).ok();
    }

    // ---------- 回退路径 ----------

    #[test]
    fn frame_source_falls_back_to_mock_when_no_assets() {
        let root = temp_root("fallback");
        let mut src = frame_source_from_assets(&root, "shenshen");
        assert_eq!(src.frame_count(), 0, "回退 Mock（空预设）");
        let f = src.next_frame(&VirtualClock::new());
        assert!(f.is_logical());
        fs::remove_dir_all(&root).ok();
    }

    // ---------- 解码线程 ----------

    #[test]
    fn decode_thread_drives_and_stops_gracefully() {
        let preset = vec![
            Frame::logical(0, 0),
            Frame::logical(33, 1),
            Frame::logical(66, 2),
        ];
        let mut playback = Playback::new(MockFrameSource::new(preset));
        let rx = playback.fanout().subscribe();

        let thread = DecodeThread::spawn(playback, Duration::from_millis(5));
        assert!(thread.is_running());
        thread::sleep(Duration::from_millis(80));

        let got = rx.try_recv().expect("线程推进期间应至少投递一帧");
        assert_eq!(got.index, 0);

        thread.stop().expect("线程应优雅退出（join Ok）");
    }
}
