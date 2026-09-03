//! ManagedAccountSwitch 的持久化端口。
//!
//! 端口只接收非敏感账号元数据，禁止凭证对象进入 application 层。

use traesync_domain::AccountProfile;

/// 账号档案存储端口。实现必须使用原子发布，不保存 token/cookie。
pub trait ManagedAccountProfileStorePort: Send + Sync {
    fn load_profiles(&self) -> Result<Vec<AccountProfile>, String>;

    /// 原子合并一个已验证档案，并返回实际已提交的完整列表。
    ///
    /// 实现必须在同一跨进程临界区内完成读取、合并、发布和回读；应用层不得自行
    /// 组合 `load_profiles` 与全量保存，否则两个实例会丢失彼此新发现的账号。
    fn upsert_profile(&self, profile: AccountProfile) -> Result<Vec<AccountProfile>, String>;
}
