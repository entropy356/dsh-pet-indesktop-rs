//! dsh-pet-indesktop-rs —— 二进制入口（骨架阶段仅打印模块划分）。
//!
//! 模块实现见 `src/lib.rs`；各阶段功能在此逐段接入。

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
    /// 骨架自检：版本号元信息非空（编译期元数据存在性）。
    #[test]
    fn skeleton_metadata_present() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
    }
}
