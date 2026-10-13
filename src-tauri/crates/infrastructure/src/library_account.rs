//! 产品×库 → 当前账号解析（P12-R1b 自组合根下沉：resolve_library_account
//! 系三分支语义收编为单一入口）。
//!
//! 口径照搬 P11-5/P11-6 现状，错误码不变：
//! - 主库：登录态 blob 实测优先，实测确认后顺带纠正注册表缓存；
//! - 副环境：注册表 current_profile_id 即权威（登录由工具托管，无实测
//!   语义——环境从未启动过 TRAE 时目录内没有登录态可实测，注册表值是
//!   唯一的账号记忆）；注册表值指向未登记/已移除账号时返回 None
//!   （无授权主体，防悬空引用）。
//! - Trae CN 只读库（融合版数据根，ADR-0035）：product-state 工具选择指针
//!   （switch 命令写入的同一状态）→ Work 签到注册表档案（P15-7 迁出旧
//!   产品池）；未选择账号或指针悬空时返回 None（前端引导先在账号页选择）。

use std::path::{Path, PathBuf};

use traesync_domain::library::{LibraryContext, MASTER_ENV_ID, TRAE_CN_LIBRARY_ID};

use crate::account_registry::{AccountRecord, AccountRegistry};
use crate::blob_keepalive::archive_login_user_id;
use crate::environment_registry::{environment_registry_error_code, EnvironmentRegistry};
use crate::product_account_state::{product_state_root_from, ProductAccountStateStore};

/// 产品×库 → 当前账号解析结果（Q1：直接 enum 锁定判别语义）。
pub enum LibraryAccountResolution {
    /// 主库：登录 blob 实测优先（解析侧已完成注册表缓存纠正副作用）。
    Measured {
        /// 实测确认的当前账号档案。
        account: AccountRecord,
    },
    /// 副库/产品池：注册表权威（副环境 current_profile_id 或 Trae CN 产品池指针）。
    Registry {
        /// 注册表指向的当前账号档案。
        account: AccountRecord,
    },
}

impl LibraryAccountResolution {
    /// 只需「账号或空」的调用点用此辅助方法，保持一行。
    pub fn into_account(self) -> Option<AccountRecord> {
        match self {
            Self::Measured { account } | Self::Registry { account } => Some(account),
        }
    }
}

/// Trae CN 产品池指针查找的三态结果：库作用域把两种空形态都映射为
/// None（无账号）；远程设备路径需区分「未选择」与「指针悬空」两个
/// 稳定错误码，故以三态承载而非复用 LibraryAccountResolution。
pub enum TraeCnPointerLookup {
    /// 池内未选择账号（指针为空）。
    NoSelection,
    /// 指针指向已登记账号。
    Registered(AccountRecord),
    /// 指针悬空（指向未登记/已移除账号）。
    Dangling,
}

/// 产品×库 → 当前账号解析单一入口（原组合根 resolve_library_account）。
pub fn resolve_library_account(
    material_root: &Path,
    storage_root: &Path,
    ctx: &LibraryContext,
) -> Result<Option<LibraryAccountResolution>, String> {
    // Trae CN 只读库（融合版数据根）：product-state 工具选择指针 → Work
    // 签到注册表档案（分支三；P15-7 迁出旧产品池，ADR-0035）。
    if ctx.env_id == TRAE_CN_LIBRARY_ID {
        return Ok(match resolve_trae_cn_pointer(
            material_root,
            &product_state_root_from(storage_root),
        )? {
            TraeCnPointerLookup::Registered(account) => {
                Some(LibraryAccountResolution::Registry { account })
            }
            // 未选择与指针悬空在库作用域同为「无账号」：前端引导先选择。
            TraeCnPointerLookup::NoSelection | TraeCnPointerLookup::Dangling => None,
        });
    }
    // 主库：登录态实测优先 + 注册表纠正（分支一）。
    if ctx.is_master() {
        return Ok(resolve_measured_env_account(
            material_root,
            storage_root,
            &ctx.dir,
            &ctx.env_id,
        )?
        .map(|account| LibraryAccountResolution::Measured { account }));
    }
    // 副环境：注册表 current_profile_id 即权威（分支二）。
    Ok(
        resolve_secondary_registry_account(material_root, storage_root, &ctx.env_id)?
            .map(|account| LibraryAccountResolution::Registry { account }),
    )
}

/// 分支一：主库/副环境目录登录态实测（原组合根 resolve_actual_env_account）。
///
/// 环境注册表是缓存，不能作为授权主体；这里要求库目录登录态 blob
/// 实测得到已登记账号。登出、blob 不可读或账号未登记时均返回 None，
/// 防止沿用旧缓存读取或修改另一账号的数据。
/// 实测已确认账号时顺手收敛该环境的注册表缓存（实测优先，主库/副环境
/// 各自记忆，互不串写）。公开供组合根主库特化入口（resolve_actual_master_account）转发。
pub fn resolve_measured_env_account(
    material_root: &Path,
    storage_root: &Path,
    instance_dir: &Path,
    env_id: &str,
) -> Result<Option<AccountRecord>, String> {
    let registry = EnvironmentRegistry::new(storage_root);
    let cached = if env_id == MASTER_ENV_ID {
        registry
            .ensure_master()
            .map_err(|_| "environment_registry_invalid".to_string())?
    } else {
        registry
            .find(env_id)
            .map_err(environment_registry_error_code)?
            .ok_or_else(|| "library_not_found".to_string())?
    };
    let accounts = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let Some(profile_id) = observed_profile_id(&accounts, instance_dir) else {
        return Ok(None);
    };
    if cached.current_profile_id.as_deref() != Some(profile_id.as_str()) {
        // 实测已确认账号时顺手收敛缓存，供环境页等展示路径使用。
        let _ = registry.set_current_profile(env_id, &profile_id);
    }
    Ok(accounts
        .into_iter()
        .find(|record| record.profile_id == profile_id))
}

/// 分支二：副环境注册表权威（current_profile_id → 主账号注册表档案）。
fn resolve_secondary_registry_account(
    material_root: &Path,
    storage_root: &Path,
    env_id: &str,
) -> Result<Option<AccountRecord>, String> {
    let profile_id = EnvironmentRegistry::new(storage_root)
        .find(env_id)
        .map_err(environment_registry_error_code)?
        .ok_or_else(|| "library_not_found".to_string())?
        .current_profile_id;
    let Some(profile_id) = profile_id else {
        return Ok(None);
    };
    let account = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?
        .into_iter()
        .find(|record| record.profile_id == profile_id);
    Ok(account)
}

/// 分支三：product-state 工具选择指针 → Work 签到注册表档案（P15-7 迁出
/// 旧产品池后：指针在 product-state 根，档案在 Work 签到注册表；融合版
/// 16 账号即档案主体，ADR-0035）。
pub fn resolve_trae_cn_pointer(
    material_root: &Path,
    state_root: &Path,
) -> Result<TraeCnPointerLookup, String> {
    let target = match ProductAccountStateStore::new(state_root).load() {
        Ok(Some(profile_id)) => profile_id,
        Ok(None) => return Ok(TraeCnPointerLookup::NoSelection),
        Err(_) => return Err("trae_cn_account_state_invalid".to_string()),
    };
    let account = AccountRegistry::new(material_root)
        .load()
        .map_err(|_| "trae_cn_registry_invalid".to_string())?
        .into_iter()
        .find(|record| record.profile_id == target);
    Ok(match account {
        Some(account) => TraeCnPointerLookup::Registered(account),
        None => TraeCnPointerLookup::Dangling,
    })
}

/// 登录态 blob 实测 userId（与 E1 存档核对同源），在账号注册表按
/// account_id 反查 profile。Some = 实测登录且账号已登记；None = 登出态
/// /blob 不可读/账号未登记。公开供组合根环境页与自检展示路径复用。
pub fn observed_profile_id(accounts: &[AccountRecord], instance_dir: &Path) -> Option<String> {
    let user_id = archive_login_user_id(instance_dir)?;
    accounts
        .iter()
        .find(|record| record.account_id == user_id)
        .map(|record| record.profile_id.clone())
}

/// Trae CN 产品账号根 = `{storage_root}/products/trae_cn`（与
/// trae_cn_material_root 同源派生规则；原组合根纯函数形态随迁）。
pub fn trae_cn_material_root_from(storage_root: &Path) -> PathBuf {
    storage_root.join("products").join("trae_cn")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blob_keepalive::{construct_auth_identity, ConstructAuthInput};
    use crate::checkin_http::UserInfoFull;
    use tempfile::tempdir;

    /// 构造测试账号档案（与原组合根 library_scope_tests 同款占位形态）。
    fn account(profile_id: &str, account_id: &str, screen_name: &str) -> AccountRecord {
        AccountRecord {
            profile_id: profile_id.to_string(),
            account_id: account_id.to_string(),
            screen_name: screen_name.to_string(),
            avatar_url: String::new(),
            device_id: format!("device-{profile_id}"),
            device_public_key: format!("pub-{profile_id}"),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 1,
            last_verified_at_unix_seconds: 1,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: false,
            archived: false,
        }
    }

    /// 在实例目录播种登录态（E2 构造路径，登出态目录全新生成 blob）。
    fn seed_login(instance_dir: &Path, account_id: &str) {
        let info = UserInfoFull {
            user_id: account_id.to_string(),
            screen_name: format!("用户{account_id}"),
            ..Default::default()
        };
        let input = ConstructAuthInput {
            account_id,
            access_token: "test-token",
            refresh_token: "test-refresh",
            access_token_expires_at_unix_seconds: 4_000_000_000,
            refresh_token_expires_at_unix_seconds: 4_100_000_000,
            user_info: &info,
        };
        construct_auth_identity(&input, instance_dir).unwrap();
    }

    /// 主库上下文（dir 即登录态实测目录）。
    fn master_ctx(dir: &Path) -> LibraryContext {
        LibraryContext {
            env_id: MASTER_ENV_ID.to_string(),
            env_name: "主库".to_string(),
            dir: dir.to_path_buf(),
            read_only: false,
        }
    }

    // ===== 分支一：主库实测优先 + 注册表缓存纠正 =====

    /// 主库实测登录已登记账号：实测值优先于过期注册表缓存，且返回
    /// Measured 变体；解析侧顺带把注册表缓存纠正为实测值。
    #[test]
    fn measured_branch_prefers_observed_login_and_corrects_registry() {
        let root = tempdir().unwrap();
        let material_root = root.path().join("material");
        let master_dir = root.path().join("master-instance");
        AccountRegistry::new(&material_root)
            .upsert(&account("checkin-a", "111", "账号A"))
            .unwrap();
        AccountRegistry::new(&material_root)
            .upsert(&account("checkin-b", "222", "账号B"))
            .unwrap();
        // 注册表缓存：账号 A（过期值）；主库实际登录：账号 B。
        EnvironmentRegistry::new(root.path())
            .set_current_profile(MASTER_ENV_ID, "checkin-a")
            .unwrap();
        seed_login(&master_dir, "222");

        let ctx = master_ctx(&master_dir);
        let resolved = resolve_library_account(&material_root, root.path(), &ctx)
            .unwrap()
            .expect("已登录且已登记的主库账号应可解析");

        // 实测优先：返回 B 的 profile，且变体为 Measured（判别语义锁定）。
        match resolved {
            LibraryAccountResolution::Measured { account } => {
                assert_eq!(account.profile_id, "checkin-b");
                assert_eq!(account.account_id, "222");
            }
            LibraryAccountResolution::Registry { .. } => {
                panic!("主库实测路径应返回 Measured 变体");
            }
        }
        // 注册表被纠正为实测值（插件同步预检等消费方随注册表一并修正）。
        let persisted = EnvironmentRegistry::new(root.path())
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(persisted.current_profile_id.as_deref(), Some("checkin-b"));
    }

    // ===== 分支二：副环境注册表权威 =====

    /// 副环境当前账号以注册表值为权威：目录内无登录态 blob（环境从未
    /// 启动过 TRAE）时仍能解析，且返回 Registry 变体。
    #[test]
    fn resolve_library_account_uses_registry_for_secondary() {
        let root = tempdir().unwrap();
        let material_root = root.path().join("material");
        let env_id = EnvironmentRegistry::new(root.path())
            .create("测试环境")
            .unwrap()
            .env_id;
        AccountRegistry::new(&material_root)
            .upsert(&account("checkin-a", "111", "账号A"))
            .unwrap();
        EnvironmentRegistry::new(root.path())
            .set_current_profile(&env_id, "checkin-a")
            .unwrap();
        let ctx = LibraryContext {
            env_id: env_id.clone(),
            env_name: "测试环境".to_string(),
            dir: root.path().join("env-dir"),
            read_only: false,
        };

        let resolved = resolve_library_account(&material_root, root.path(), &ctx)
            .unwrap()
            .expect("副环境注册表登记账号应直接解析（不依赖目录登录态实测）");
        match resolved {
            LibraryAccountResolution::Registry { account } => {
                assert_eq!(account.profile_id, "checkin-a");
                assert_eq!(account.account_id, "111");
            }
            LibraryAccountResolution::Measured { .. } => {
                panic!("副环境注册表路径应返回 Registry 变体");
            }
        }
    }

    /// 副环境未登记登录账号（注册表无值）：读取与归属操作无法确定目标 → None。
    #[test]
    fn resolve_library_account_returns_none_for_unlogged_secondary() {
        let root = tempdir().unwrap();
        let env_id = EnvironmentRegistry::new(root.path())
            .create("测试环境")
            .unwrap()
            .env_id;
        let ctx = LibraryContext {
            env_id,
            env_name: "测试环境".to_string(),
            dir: root.path().join("env-dir"),
            read_only: false,
        };

        let resolved =
            resolve_library_account(&root.path().join("material"), root.path(), &ctx).unwrap();
        assert!(resolved.is_none());
    }

    /// 副环境注册表值指向未登记账号（账号已从账号页移除）：无授权主体
    /// → None，与主库「登录未登记账号 → None」同口径，防悬空引用。
    #[test]
    fn resolve_library_account_returns_none_for_unknown_profile() {
        let root = tempdir().unwrap();
        let env_id = EnvironmentRegistry::new(root.path())
            .create("测试环境")
            .unwrap()
            .env_id;
        EnvironmentRegistry::new(root.path())
            .set_current_profile(&env_id, "checkin-gone")
            .unwrap();
        let ctx = LibraryContext {
            env_id,
            env_name: "测试环境".to_string(),
            dir: root.path().join("env-dir"),
            read_only: false,
        };

        let resolved =
            resolve_library_account(&root.path().join("material"), root.path(), &ctx).unwrap();
        assert!(resolved.is_none());
    }

    // ===== 分支三：融合版 product-state 工具选择指针 =====

    /// Trae CN 只读库（融合版数据根）当前账号 = product-state 工具选择指针
    /// → Work 签到注册表档案（与 switch_trae_cn_account 同源；P15-7 迁出
    /// 旧产品池），返回 Registry 变体。
    #[test]
    fn resolve_library_account_uses_fused_product_pointer() {
        let root = tempdir().unwrap();
        let checkin_root = root.path().join("checkin");
        AccountRegistry::new(&checkin_root)
            .upsert(&account("checkin-cn-1", "888", "融合版账号"))
            .unwrap();
        ProductAccountStateStore::new(product_state_root_from(root.path()))
            .save("checkin-cn-1")
            .unwrap();
        let ctx = LibraryContext {
            env_id: TRAE_CN_LIBRARY_ID.to_string(),
            env_name: "Trae CN".to_string(),
            dir: root.path().to_path_buf(),
            read_only: true,
        };

        let resolved = resolve_library_account(&checkin_root, root.path(), &ctx)
            .unwrap()
            .expect("指针指向的注册表账号应直接解析");
        match resolved {
            LibraryAccountResolution::Registry { account } => {
                assert_eq!(account.profile_id, "checkin-cn-1");
                assert_eq!(account.account_id, "888");
            }
            LibraryAccountResolution::Measured { .. } => {
                panic!("融合版指针路径应返回 Registry 变体");
            }
        }
    }

    /// 未选择账号（product-state 无指针）：None——前端引导先在账号页选择。
    #[test]
    fn resolve_library_account_returns_none_for_unselected_trae_cn() {
        let root = tempdir().unwrap();
        let ctx = LibraryContext {
            env_id: TRAE_CN_LIBRARY_ID.to_string(),
            env_name: "Trae CN".to_string(),
            dir: root.path().to_path_buf(),
            read_only: true,
        };

        let resolved =
            resolve_library_account(&root.path().join("checkin"), root.path(), &ctx).unwrap();
        assert!(resolved.is_none());
    }

    /// 指针悬空（指向未登记/已移除账号）：None——无授权主体，防悬空引用。
    #[test]
    fn resolve_library_account_returns_none_for_dangling_trae_cn_pointer() {
        let root = tempdir().unwrap();
        ProductAccountStateStore::new(product_state_root_from(root.path()))
            .save("checkin-gone")
            .unwrap();
        let ctx = LibraryContext {
            env_id: TRAE_CN_LIBRARY_ID.to_string(),
            env_name: "Trae CN".to_string(),
            dir: root.path().to_path_buf(),
            read_only: true,
        };

        let resolved =
            resolve_library_account(&root.path().join("checkin"), root.path(), &ctx).unwrap();
        assert!(resolved.is_none());
    }

    /// 指针查找的三态契约：库作用域把「未选择」与「悬空」都映射为 None，
    /// 但共用函数仍区分三态（稳定错误码口径保留）。
    #[test]
    fn trae_cn_pointer_lookup_distinguishes_unselected_registered_and_dangling() {
        let root = tempdir().unwrap();
        let checkin_root = root.path().join("checkin");
        AccountRegistry::new(&checkin_root)
            .upsert(&account("checkin-cn-1", "888", "融合版账号"))
            .unwrap();
        let state_root = product_state_root_from(root.path());

        // 未选择：product-state 无指针 → NoSelection。
        assert!(matches!(
            resolve_trae_cn_pointer(&checkin_root, &state_root).unwrap(),
            TraeCnPointerLookup::NoSelection
        ));
        // 指针指向已登记账号 → Registered。
        ProductAccountStateStore::new(&state_root)
            .save("checkin-cn-1")
            .unwrap();
        match resolve_trae_cn_pointer(&checkin_root, &state_root).unwrap() {
            TraeCnPointerLookup::Registered(record) => {
                assert_eq!(record.profile_id, "checkin-cn-1");
            }
            _ => panic!("指针指向已登记账号应返回 Registered"),
        }
        // 悬空：指针指向未登记 profile → Dangling。
        ProductAccountStateStore::new(&state_root)
            .save("checkin-gone")
            .unwrap();
        assert!(matches!(
            resolve_trae_cn_pointer(&checkin_root, &state_root).unwrap(),
            TraeCnPointerLookup::Dangling
        ));
    }

    // ===== into_account 辅助方法两态 =====

    /// 两个变体经 into_account 均取出同一档案（调用方无需 match 即得账号）。
    #[test]
    fn into_account_maps_both_variants_to_account() {
        let record = account("checkin-a", "111", "账号A");
        let measured = LibraryAccountResolution::Measured {
            account: record.clone(),
        };
        let registry = LibraryAccountResolution::Registry {
            account: record.clone(),
        };
        assert_eq!(measured.into_account().unwrap().profile_id, "checkin-a");
        assert_eq!(registry.into_account().unwrap().profile_id, "checkin-a");
    }
}
