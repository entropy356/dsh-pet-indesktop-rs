# dsh-pet-indesktop-rs

<p align="center">
  <img alt="语言" src="https://img.shields.io/badge/语言-Rust-DEA584">
  <img alt="平台" src="https://img.shields.io/badge/平台-Windows%20%7C%20macOS%20%7C%20Linux-8A2BE2">
</p>

将 [MerZlin/dsh-pet-indesktop](https://github.com/MerZlin/dsh-pet-indesktop)（Python + PySide6 桌面宠物「蓝色大肥鱼」）用 **Rust** 重构的工程。

> 原项目亮点：透明无边框、置顶、可拖动的桌宠窗口；角色切换与动画播放；系统托盘；单进程多开（共享解码链）；物理/碰撞彩蛋；可选 AI 对话能力。本仓库目标是在保持功能与交互体验的前提下，用 Rust 获得更小的体积、更低的内存与更快的启动。

## 当前进度：第一阶段（骨架）

- ✅ 模块划分与占位接口（`window` / `animation` / `physics` / `tray` / `config`）
- ✅ 零依赖、开箱即编译，内置单元测试
- ⬜ 第二阶段：winit 透明置顶窗口 + 拖动 + 动画帧渲染
- ⬜ 第三阶段：系统托盘、右键菜单、配置持久化
- ⬜ 第四阶段：单进程多开与共享解码链
- ⬜ 第五阶段：AI 对话、语音、音乐歌词等扩展能力

## 技术选型（规划）

| 领域 | 原项目（Python） | Rust 方案 |
|---|---|---|
| 窗口/事件 | PySide6 | `winit` |
| 渲染 | Qt 绘制 | `softbuffer`（起步）→ `wgpu`（加速） |
| 托盘 | Qt SystemTray | `tray-icon` |
| 配置 | JSON | `serde` + `serde_json`/`toml` |
| 视频解码 | OpenCV/Qt Multimedia | `ffmpeg-next` 或平台原生解码 |
| 纯逻辑层 | collision.py / physics.py | 本仓 `src/physics.rs`（零 GUI 依赖，单测守护） |

## 架构红线（继承自原项目，CI 守护）

1. **纯逻辑层不依赖 GUI**：`src/physics.rs` 等纯逻辑模块禁止引入 winit/wgpu 等渲染依赖。
2. **共享解码链单向依赖**：`animation` 不得反向依赖 `window`；窗口钩子只能通过注入接入。
3. **窗口私有面冻结**：`PetWindow` 的内部字段仅 `window` 模块自身可访问。

## 开发

```bash
cargo build        # 编译
cargo test         # 运行单元测试
cargo run          # 运行（骨架阶段仅打印模块划分）
```

## 许可证

待定（建议与原项目保持一致，迁移前查阅原仓库 LICENSE）。

## 致谢

- 原项目与「蓝色大肥鱼」素材：[MerZlin/dsh-pet-indesktop](https://github.com/MerZlin/dsh-pet-indesktop)（基于 PC2005-cloud/dsh-pet）
