//! dsh-pet-indesktop-rs
//!
//! Rust 重构版桌面宠物（原项目：https://github.com/MerZlin/dsh-pet-indesktop）。
//! 目标：透明无边框、置顶、可拖动的桌宠窗口，支持角色切换、动画播放、
//! 系统托盘、多开与可选 AI 对话能力，覆盖 Windows / Linux / macOS。
//!
//! 当前处于第一阶段（骨架）：模块划分与接口占位已就绪，零依赖可编译。
//! 路线图见 README.md。

mod animation;
mod config;
mod physics;
mod tray;
mod window;

fn main() {
    println!("dsh-pet-indesktop-rs v{}", env!("CARGO_PKG_VERSION"));
    println!("Rust 重构版桌面宠物 —— 骨架阶段");
    println!();
    println!("模块划分：");
    for module in [
        "window     桌宠窗口（透明、置顶、拖动、多窗）",
        "animation  动画播放（解码链 + 帧调度）",
        "physics    纯物理/碰撞逻辑层（不依赖 GUI）",
        "tray       系统托盘与右键菜单",
        "config     配置加载/保存与多开 slot 作用域",
    ] {
        println!("  - {module}");
    }
    println!();
    println!("详见 README.md 路线图。");
}

#[cfg(test)]
mod tests {
    #[test]
    fn skeleton_compiles() {
        assert!(true);
    }
}
