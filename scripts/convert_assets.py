#!/usr/bin/env python3
r"""convert_assets.py —— webm → PNG 序列帧转换工具（issue #14）。

把原项目 dsh-pet-indesktop 的角色视频素材（assets/characters/<角色>/videos/
<行为分类>/**.webm）转换为 Rust 版运行时使用的 PNG 序列帧，并为每个片段
生成 manifest.json（schema 见仓库 README「素材」节 / issue #14 定稿基线）。

# 目录约定（约定权威来源：issue #14）

    源（原项目）:  assets/characters/shenshen/videos/<分类层级>/<片段>.webm
    输出（本仓库）: assets/shenshen/<分类层级>/<片段>/NNNN.png + manifest.json

原项目行为分类目录（click / random / events/balance 等）下常有多个视频，
因此**每个视频对应一个输出目录**（片段名 = 原文件名去扩展名），帧名从
0000.png 起零填充 4 位递增。加载器（issue #12）按「含 manifest.json 的
目录 = 一个可播放片段」递归扫描；分类取其相对路径父级。

# 用法示例

    # 完整转换（默认输出到 ./assets/，帧率上限 24fps，保持原分辨率）
    python3 scripts/convert_assets.py --src /path/to/dsh-pet-indesktop/assets/characters/shenshen/videos

    # 只转 idle 分类，帧率压到 12fps，输出到指定目录，缩放到 300x300
    python3 scripts/convert_assets.py --src .../videos --include idle --dst /tmp/out --fps-cap 12 --size 300x300

# 依赖

Python 3.8+ 标准库 + 系统 ffmpeg / ffprobe（Debian/Ubuntu: apt install ffmpeg；
macOS: brew install ffmpeg）。素材本体不入库（.gitignore 已覆盖生成物）。

# 授权提示

角色素材为同人作品（CC BY-NC-SA 类授权，见 THIRD_PARTY_NOTICES.md），
仅限个人非商业使用，须保留署名与来源；转换产物同受其约束。
"""

from __future__ import annotations

import argparse
import json
import math
import re
import shutil
import subprocess
import sys
from pathlib import Path

MANIFEST_VERSION = 1
VIDEO_EXTS = {".webm"}  # 原项目素材格式；后续格式扩展在此追加


def die(msg: str) -> "NoReturn":  # type: ignore[valid-type]
    print(f"[convert_assets] 错误：{msg}", file=sys.stderr)
    sys.exit(1)


def require_ffmpeg() -> None:
    """启动探测 ffmpeg / ffprobe，缺失时清晰报错并给安装提示。"""
    missing = [t for t in ("ffmpeg", "ffprobe") if shutil.which(t) is None]
    if missing:
        die(
            f"未找到 {'、'.join(missing)}。请先安装 ffmpeg：\n"
            "  Debian/Ubuntu: sudo apt-get install ffmpeg\n"
            "  macOS:         brew install ffmpeg\n"
            "  Windows:       winget install Gyan.FFmpeg（或下载静态构建加入 PATH）"
        )


def ffprobe_json(path: Path) -> dict:
    proc = subprocess.run(
        ["ffprobe", "-v", "error", "-print_format", "json",
         "-show_streams", "-show_format", str(path)],
        capture_output=True, text=True,
    )
    if proc.returncode != 0:
        die(f"ffprobe 解析失败 {path.name}：{proc.stderr.strip()[:300]}")
    return json.loads(proc.stdout)


def source_fps(probe: dict) -> float:
    """从 ffprobe 流信息取帧率（r_frame_rate 形如 '30000/1001'）。"""
    for stream in probe.get("streams", []):
        if stream.get("codec_type") == "video":
            num, _, den = stream.get("r_frame_rate", "0/1").partition("/")
            try:
                fps = float(num) / float(den or 1)
            except (ValueError, ZeroDivisionError):
                fps = 0.0
            if fps > 0:
                return fps
    return 0.0


def parse_size(text: str) -> tuple[int, int] | None:
    """解析 WxH；返回 None 表示保持原分辨率。"""
    if text is None:
        return None
    m = re.fullmatch(r"(\d+)[xX](\d+)", text.strip())
    if not m:
        die(f"--size 格式应为 WxH（如 300x300），收到：{text}")
    w, h = int(m.group(1)), int(m.group(2))
    if w <= 0 or h <= 0:
        die(f"--size 必须为正数，收到：{text}")
    return w, h


def convert_clip(video: Path, out_dir: Path, fps: int, size: tuple[int, int] | None) -> dict:
    """转换单个视频 → out_dir/NNNN.png + manifest.json，返回 manifest 字典。"""
    probe = ffprobe_json(video)
    src_fps = source_fps(probe)
    actual_fps = min(int(round(src_fps)), fps) if src_fps > 0 else fps
    if actual_fps < 1:
        actual_fps = 1

    out_dir.mkdir(parents=True, exist_ok=True)
    cmd = ["ffmpeg", "-v", "error", "-y", "-i", str(video)]
    if size is not None:
        cmd += ["-vf", f"scale={size[0]}:{size[1]}"]
    # 约定：帧名 0000.png 起零填充 4 位（ffmpeg 默认从 0001 开始，须显式归零）
    cmd += ["-start_number", "0", "-r", str(actual_fps), str(out_dir / "%04d.png")]
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        die(f"ffmpeg 转换失败 {video.name}：{proc.stderr.strip()[:300]}")

    frames = sorted(out_dir.glob("*.png"))
    if not frames:
        die(f"转换未产出任何帧：{video.name}")
    # 帧名必须 0000 起零填充 4 位且不断号
    for i, f in enumerate(frames):
        if f.name != f"{i:04d}.png":
            die(f"帧序断号：{out_dir} 中 {f.name}（期望 {i:04d}.png）")

    # 帧尺寸从实际产物读取（--size 可能改写）
    probe_out = ffprobe_json(frames[0])
    stream = next(s for s in probe_out["streams"] if s.get("codec_type") == "video")
    manifest = {
        "version": MANIFEST_VERSION,
        "fps": actual_fps,
        "frame_count": len(frames),
        "width": int(stream["width"]),
        "height": int(stream["height"]),
    }
    (out_dir / "manifest.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return manifest


def main() -> None:
    ap = argparse.ArgumentParser(
        description="webm → PNG 序列帧转换工具（issue #14，#12 加载器的约定来源）",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__.split("用法示例")[1].split("# 依赖")[0] if __doc__ else None,
    )
    ap.add_argument("--src", required=True,
                    help="原项目 videos 目录（如 .../assets/characters/shenshen/videos）")
    ap.add_argument("--dst", default="assets/",
                    help="输出根目录（默认 assets/，输出 <dst>/<角色>/<分类>/<片段>/）")
    ap.add_argument("--fps-cap", type=int, default=24, metavar="N",
                    help="播放帧率上限（默认 24，降帧省磁盘；manifest.fps ≤ N）")
    ap.add_argument("--size", metavar="WxH", default=None,
                    help="目标尺寸（如 300x300；默认保持原素材分辨率）")
    ap.add_argument("--character", default="shenshen",
                    help="角色名（默认 shenshen，与 config 的 character 字段对齐）")
    ap.add_argument("--include", action="append", metavar="CAT",
                    help="只转换指定顶层分类（如 idle；可多次给出）")
    ap.add_argument("--force", action="store_true",
                    help="覆盖已存在的输出（默认跳过已转换片段）")
    args = ap.parse_args()

    require_ffmpeg()
    if args.fps_cap < 1:
        die("--fps-cap 必须 ≥ 1")
    size = parse_size(args.size)

    src = Path(args.src)
    if not src.is_dir():
        die(f"--src 不存在或不是目录：{src}")
    dst_root = Path(args.dst) / args.character

    videos = sorted(p for p in src.rglob("*")
                    if p.is_file() and p.suffix.lower() in VIDEO_EXTS)
    if args.include:
        inc = set(args.include)
        videos = [v for v in videos if v.relative_to(src).parts[0] in inc]
    if not videos:
        die(f"未在 {src} 找到可转换视频（{'/'.join(sorted(VIDEO_EXTS))}）"
            + (f"，include={args.include}" if args.include else ""))

    total = 0
    for v in videos:
        rel = v.relative_to(src)  # 如 click/点击回应-傲娇生气.webm
        clip_dir = dst_root / rel.with_suffix("")
        if not args.force and (clip_dir / "manifest.json").exists():
            print(f"跳过（已存在）：{clip_dir}  （--force 覆盖）")
            continue
        m = convert_clip(v, clip_dir, args.fps_cap, size)
        total += m["frame_count"]
        print(f"完成：{rel}  →  {clip_dir}/  "
              f"({m['frame_count']} 帧, {m['fps']}fps, {m['width']}x{m['height']})")

    print(f"\n共 {len(videos)} 个片段、{total} 帧。输出根：{dst_root}")
    print("提示：素材与生成物不入库（.gitignore 已覆盖）；"
          "角色素材授权 CC BY-NC-SA，仅限个人非商业使用，须保留署名与来源。")


if __name__ == "__main__":
    main()
