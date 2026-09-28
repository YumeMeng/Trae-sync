//! TRAE 产品差异的最小静态适配边界。
//!
//! 这里仅放已经通过本机安装包或只读数据确认的本地产品事实：官方数据根目录、
//! 对话库相对路径、可执行文件名和原生账号目录规则。OAuth/签到客户端参数
//! 仍由 `checkin_http` 的实测协议模块负责，不能在这里凭产品名称推断。

use std::path::Path;

/// 当前支持识别的 TRAE 产品形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraeProduct {
    /// 项目当前默认目标；本机产品名为 TRAE SOLO CN。
    WorkCn,
    /// 独立的 Trae CN IDE。
    TraeCn,
}

impl TraeProduct {
    /// 稳定产品 ID，供工作台上下文和前端路由使用。
    pub const fn product_id(self) -> &'static str {
        match self {
            Self::WorkCn => "work_cn",
            Self::TraeCn => "trae_cn",
        }
    }

    /// 面向用户的产品名称；不把安装包内部 packageType 直接暴露到 UI。
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::WorkCn => "TRAE Work CN",
            Self::TraeCn => "Trae CN",
        }
    }

    /// 官方用户数据根目录名（位于 `%APPDATA%` 下）。
    pub const fn data_root_name(self) -> &'static str {
        match self {
            Self::WorkCn => "TRAE SOLO CN",
            Self::TraeCn => "Trae CN",
        }
    }

    /// 产品内置对话库相对路径。
    pub const fn database_relative_path(self) -> &'static str {
        // 两个产品当前使用同一套 ModularData 布局；路径变化应在这里集中调整。
        "ModularData/ai-agent/database.db"
    }

    /// 安装包主程序文件名。
    pub const fn executable_file_name(self) -> &'static str {
        self.executable_file_names()[0]
    }

    /// 主程序文件名及已确认的兼容旧名；顺序第一项是当前安装包名称。
    pub const fn executable_file_names(self) -> &'static [&'static str] {
        match self {
            // Work CN 旧版仍使用 trae.exe，默认入口必须继续识别它。
            Self::WorkCn => &["TRAE SOLO CN.exe", "trae.exe"],
            Self::TraeCn => &["Trae CN.exe"],
        }
    }

    /// 注册表显示名匹配片段。
    pub const fn install_name_fragment(self) -> &'static str {
        match self {
            Self::WorkCn => "solo",
            Self::TraeCn => "trae cn",
        }
    }

    /// 已确认的原生账号目录前缀；未确认时返回 None，调用方必须安全地跳过。
    pub const fn native_account_dir_prefix(self) -> Option<&'static str> {
        match self {
            Self::WorkCn => Some("TRAE SOLO CN"),
            // 本机 Trae CN 未发现同类账号目录，暂不猜目录命名规则。
            Self::TraeCn => None,
        }
    }

    /// 判断路径是否为该产品的主程序。
    pub fn matches_executable(self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        self.executable_file_names()
            .iter()
            .any(|expected| name.eq_ignore_ascii_case(expected))
    }

    /// 判断注册表安装名是否属于该产品。
    pub fn matches_install_name(self, display_name: &str) -> bool {
        let lowered = display_name.to_ascii_lowercase();
        match self {
            Self::WorkCn => {
                lowered.contains("work") || lowered.contains(self.install_name_fragment())
            }
            Self::TraeCn => lowered.contains(self.install_name_fragment()),
        }
    }
}

impl Default for TraeProduct {
    fn default() -> Self {
        Self::WorkCn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_paths_are_separate_but_database_layout_is_shared() {
        assert_eq!(TraeProduct::WorkCn.data_root_name(), "TRAE SOLO CN");
        assert_eq!(TraeProduct::TraeCn.data_root_name(), "Trae CN");
        assert_eq!(
            TraeProduct::WorkCn.database_relative_path(),
            TraeProduct::TraeCn.database_relative_path()
        );
    }

    #[test]
    fn executable_matching_does_not_cross_select_products() {
        assert!(TraeProduct::WorkCn
            .matches_executable(Path::new(r"E:\software\TRAE SOLO CN\TRAE SOLO CN.exe")));
        assert!(TraeProduct::WorkCn.matches_executable(Path::new(r"E:\software\TRAE\TRAE.exe")));
        assert!(TraeProduct::TraeCn
            .matches_executable(Path::new(r"E:\software\TRAE\Trae CN\Trae CN.exe")));
        assert!(!TraeProduct::WorkCn
            .matches_executable(Path::new(r"E:\software\TRAE\Trae CN\Trae CN.exe")));
    }

    #[test]
    fn unknown_cn_native_account_rule_fails_closed() {
        assert_eq!(
            TraeProduct::WorkCn.native_account_dir_prefix(),
            Some("TRAE SOLO CN")
        );
        assert_eq!(TraeProduct::TraeCn.native_account_dir_prefix(), None);
    }

    #[test]
    fn product_identity_and_display_name_are_stable() {
        assert_eq!(TraeProduct::WorkCn.product_id(), "work_cn");
        assert_eq!(TraeProduct::WorkCn.display_name(), "TRAE Work CN");
        assert_eq!(TraeProduct::TraeCn.product_id(), "trae_cn");
        assert_eq!(TraeProduct::TraeCn.display_name(), "Trae CN");
    }
}
