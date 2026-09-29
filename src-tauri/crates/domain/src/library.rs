//! 库实例运行上下文与库环境稳定标识（P12-R1b 自组合根下沉）。
//!
//! ADR-0025：库 = 主库 / 副环境 / Trae CN 只读产品库的统一抽象。本模块
//! 只承载纯领域概念——环境归属标识常量、运行上下文值对象、写守卫规则；
//! 依赖实例进程接口（关停/重启/命令行匹配）的能力以扩展 trait 留在组合根。

use std::path::PathBuf;

/// 主库环境标识（Q7：默认环境，不可删除/不可修改类型）。
pub const MASTER_ENV_ID: &str = "master";

/// Trae CN 只读产品库的固定库 id（P11-6 / ADR-0032 决策 5：不入环境
/// 注册表的固定注册项；命名对齐产品 id `TraeProduct::TraeCn.product_id()`，
/// 与 `master` / `env-*` 同一 id 空间）。
pub const TRAE_CN_LIBRARY_ID: &str = "trae_cn";

/// 库实例运行上下文：写库类命令（收编/清理/归档应用）按环境
/// 路由实例生命周期（关/启/运行检测）与备份目录的统一依据。
/// P11-6 追加 `read_only`：Trae CN 产品库只读（ADR-0032 决策 5），写库
/// 命令经 `ensure_writable` 统一拒绝。
pub struct LibraryContext {
    /// 环境归属（`master` / `trae_cn` / 副环境 env_id），进度事件与台账路由用。
    pub env_id: String,
    /// 环境显示名（副环境重启窗口标题与主库「当前账号名」口径对齐用）。
    pub env_name: String,
    /// 库数据目录（主库 = 官方目录；副环境 = 环境派生目录；trae_cn = 产品官方数据根）。
    pub dir: PathBuf,
    /// 是否只读库（P11-6：仅 trae_cn 为 true；写库类命令先经 ensure_writable）。
    pub read_only: bool,
}

impl LibraryContext {
    pub fn is_master(&self) -> bool {
        self.env_id == MASTER_ENV_ID
    }

    /// P11-6（ADR-0032 决策 5）：写库类能力不随 Trae CN 只读浏览开放。
    /// 归属改写/归档/删除/合并/清理对只读库一律返回稳定错误码
    /// `library_read_only`，在进入任何实例关闭或备份动作之前拒绝。
    pub fn ensure_writable(&self) -> Result<(), String> {
        if self.read_only {
            Err("library_read_only".to_string())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 写守卫：trae_cn 库返回 library_read_only；主库/副环境不受影响
    ///（原组合根 library_write_guard_rejects_trae_cn_only 语义随迁）。
    #[test]
    fn library_write_guard_rejects_trae_cn_only() {
        let read_only = LibraryContext {
            env_id: TRAE_CN_LIBRARY_ID.to_string(),
            env_name: "Trae CN".to_string(),
            dir: PathBuf::from("C:/fixture/Trae CN"),
            read_only: true,
        };
        assert_eq!(
            read_only.ensure_writable().unwrap_err(),
            "library_read_only"
        );

        let master = LibraryContext {
            env_id: MASTER_ENV_ID.to_string(),
            env_name: "主库".to_string(),
            dir: PathBuf::from("C:/fixture/master"),
            read_only: false,
        };
        let secondary = LibraryContext {
            env_id: "env-1-a".to_string(),
            env_name: "副环境".to_string(),
            dir: PathBuf::from("C:/fixture/env-1-a"),
            read_only: false,
        };
        assert!(master.ensure_writable().is_ok());
        assert!(secondary.ensure_writable().is_ok());
    }

    /// is_master 仅对主库环境标识为真；trae_cn 与副环境均不算主库。
    #[test]
    fn is_master_matches_only_master_env_id() {
        let master = LibraryContext {
            env_id: MASTER_ENV_ID.to_string(),
            env_name: "主库".to_string(),
            dir: PathBuf::from("C:/fixture/master"),
            read_only: false,
        };
        assert!(master.is_master());

        let trae_cn = LibraryContext {
            env_id: TRAE_CN_LIBRARY_ID.to_string(),
            env_name: "Trae CN".to_string(),
            dir: PathBuf::from("C:/fixture/Trae CN"),
            read_only: true,
        };
        assert!(!trae_cn.is_master());
    }
}
