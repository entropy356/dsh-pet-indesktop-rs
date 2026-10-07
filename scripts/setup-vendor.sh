#!/usr/bin/env bash
# 一次性搭建离线依赖（vendor）：下载 → SHA256 校验 → 解包 → 写 .cargo/config.toml。
# 之后 cargo 全程离线（vendored-sources），不触碰 crates.io。
#
# 用法：bash scripts/setup-vendor.sh
# 下载通道自动降级：直连 GitHub release → ghfast.top 前缀代理。
# 代理通道的安全性由 SHA256SUMS 校验闭环保证（篡改即失败退出）。
set -euo pipefail

VENDOR_TAG="vendor-c4b61c75"
BASE="https://github.com/entropy356/portable-toolchain-pack/releases/download/${VENDOR_TAG}"
GHFAST="https://ghfast.top"

command -v curl >/dev/null 2>&1 || { echo "缺少 curl" >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { echo "缺少 sha256sum" >&2; exit 1; }
command -v tar >/dev/null 2>&1 || { echo "缺少 tar" >&2; exit 1; }
command -v xz >/dev/null 2>&1 || { echo "缺少 xz-utils（tar -J 解压需要）" >&2; exit 1; }

# 制品名与 tag 严格配套（release 资产名即 <tag>.tar.xz，lock 变更换 tag 时自动同步）
TARBALL="${VENDOR_TAG}.tar.xz"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

try_download() {  # $1 = URL 前缀
    local url="$1/${BASE#https://}"
    [ "$1" = "https://github.com" ] && url="$BASE/$TARBALL" || url="$1/${BASE}/$TARBALL"
    echo "尝试通道：$url"
    curl -fL --max-time 300 -o "$WORK/$TARBALL" "$url" 2>/dev/null
}

# 通道降级链：直连 → ghfast
DOWNLOADED=0
for prefix in "https://github.com" "$GHFAST"; do
    if try_download "$prefix"; then DOWNLOADED=1; echo "通道成功：$prefix"; break; fi
    echo "通道失败，降级…"
done
[ "$DOWNLOADED" = "1" ] || { echo "错误：所有下载通道均失败（直连与 ghfast 代理）" >&2; exit 1; }

# SHA256 校验（与 release 的 SHA256SUMS 同源；此处内联锁定防篡改）
expect="c94f6956499e70818e8c57498dcaa888e326960707f46a9acb9cb9a9e93eff52  $TARBALL"
actual="$(cd "$WORK" && sha256sum "$TARBALL")"
[ "$actual" = "$expect" ] || { echo "校验失败：$actual ≠ $expect" >&2; exit 1; }
echo "SHA256 校验通过"

# 解包到仓库根
tar -xJf "$WORK/$TARBALL" -C "$(pwd)"

# 写本地 cargo 配置（vendor 生效；目录不进 git，由 .gitignore 覆盖）
mkdir -p .cargo
cat > .cargo/config.toml <<'CFG'
# 离线依赖：全部走本地 vendor（由 scripts/setup-vendor.sh 生成，勿提交）
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
CFG

echo
echo "完成。验证：cargo build --offline && cargo test --offline"
