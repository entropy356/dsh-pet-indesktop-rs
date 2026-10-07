#!/usr/bin/env bash
# 架构红线守护脚本（README「架构红线」一节的机器可执行版本）。
# CI 中执行（.github/workflows/ci.yml 的 arch job），违规即退出非零。
#
# 规则：
#   R1 纯逻辑层不依赖 GUI：src/physics.rs、src/animation.rs、src/config.rs
#      不得 use winit / wgpu / softbuffer / tray_icon
#   R2 共享解码链单向依赖：src/animation.rs 不得 use crate::window
set -euo pipefail

cd "$(dirname "$0")/.."

fail=0

# 纯逻辑模块清单（红线 1 的保护对象）。模块拆目录后在此追加，如 physics/。
PURE_MODULES=(
  "src/physics.rs"
  "src/animation.rs"
  "src/config.rs"
  "src/sim.rs"
)

GUI_PATTERNS='winit|wgpu|softbuffer|tray_icon'

# R1：纯逻辑模块禁止 GUI 依赖
for f in "${PURE_MODULES[@]}"; do
  if [ ! -f "$f" ]; then
    echo "[check-arch] 缺少受保护模块文件：$f（模块结构变更请同步更新本脚本）" >&2
    fail=1
    continue
  fi
  if grep -nP "^\s*use\s+(${GUI_PATTERNS})" "$f"; then
    echo "[check-arch] 违反红线 1：$f 引入了 GUI 依赖（winit/wgpu/softbuffer/tray_icon）" >&2
    fail=1
  fi
done

# R2：animation 不得反向依赖 window
if [ -f src/animation.rs ] && grep -nP '^\s*use\s+crate::window' src/animation.rs; then
  echo "[check-arch] 违反红线 2：src/animation.rs 反向依赖 crate::window" >&2
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  echo "[check-arch] 架构红线检查未通过" >&2
  exit 1
fi

echo "[check-arch] 架构红线检查通过（R1 纯逻辑无 GUI 依赖 / R2 animation 无 window 反向依赖）"
