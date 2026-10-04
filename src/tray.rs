//! 系统托盘与右键菜单模块。
//!
//! 对应原项目托盘 / `context_menus/`。Rust 选型（路线图第三阶段落地）：
//! - `tray-icon` crate（Windows/macOS/Linux(Ayatana/AppIndicator) 均支持）
//! - 菜单项数据结构与渲染解耦，便于"右键菜单可编排"的配置化

/// 菜单项占位。
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    /// 普通动作项（label）
    Action(String),
    /// 子菜单（label, 子项）
    Submenu(String, Vec<MenuItem>),
    /// 分隔线
    Separator,
}

/// 托盘状态占位。
pub struct Tray {
    pub menu: Vec<MenuItem>,
}

impl Tray {
    /// 默认菜单骨架，对应原项目的常用入口。
    pub fn default_menu() -> Vec<MenuItem> {
        vec![
            MenuItem::Action("显示/隐藏 桌宠".into()),
            MenuItem::Submenu("角色".into(), vec![MenuItem::Action("默认肥鱼".into())]),
            MenuItem::Separator,
            MenuItem::Action("设置".into()),
            MenuItem::Action("退出".into()),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_menu_has_exit() {
        let menu = Tray::default_menu();
        assert!(menu.iter().any(|item| matches!(item, MenuItem::Action(s) if s == "退出")));
    }
}
