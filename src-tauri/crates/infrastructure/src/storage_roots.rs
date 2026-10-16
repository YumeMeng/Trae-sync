//! P16-T2：工具存储根下的派生路径**唯一来源**（ADR-0035 路径约定）。
//!
//! 四个根目录的拼接规则此前散落在 library_account / product_account_state /
//! checkin_schedule / credential_handover 各自为政——移植到新宿主产品时需要
//! 逐处核对。本模块收口后，新平台适配只需改动此文件一处。
//!
//! 全部为纯函数：不落盘、无副作用、不触碰凭据材料；路径约定见 ADR-0035。

use std::path::{Path, PathBuf};

/// 签到/账号池材料根：`{storage_root}/checkin`。
/// OAuth 登录入库与真实签到必须使用同一路径约定，避免档案与凭据分离。
pub fn checkin_material_root_from(storage_root: &Path) -> PathBuf {
    storage_root.join("checkin")
}

/// 旧 CN 产品池根：`{storage_root}/products/trae_cn`。
/// P15-7 后已无生产写入方（登录入库链待迁移），保留供历史数据读取与清理。
pub fn trae_cn_material_root_from(storage_root: &Path) -> PathBuf {
    storage_root.join("products").join("trae_cn")
}

/// 产品账号选择指针根：`{storage_root}/product-state`。
/// 迁到独立目录，避免指针写入复活已删除目录。
pub fn product_state_root_from(storage_root: &Path) -> PathBuf {
    storage_root.join("product-state")
}

/// 融合版登录代次容器根：`{storage_root}/fused-login`（P15-4，ADR-0035）。
/// 与阶段一迁移探针的落盘位置一致，容器由 FusedLoginStore 管理。
pub fn fused_login_root_from(storage_root: &Path) -> PathBuf {
    storage_root.join("fused-login")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_follow_adr_0035_layout() {
        let root = Path::new("S");
        assert_eq!(
            checkin_material_root_from(root),
            PathBuf::from("S").join("checkin")
        );
        assert_eq!(
            trae_cn_material_root_from(root),
            PathBuf::from("S").join("products").join("trae_cn")
        );
        assert_eq!(
            product_state_root_from(root),
            PathBuf::from("S").join("product-state")
        );
        assert_eq!(
            fused_login_root_from(root),
            PathBuf::from("S").join("fused-login")
        );
    }
}
