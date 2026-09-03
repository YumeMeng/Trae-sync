//! 持久承接意图端口。
//!
//! 端口只接收稳定选择和证据摘要；实现不得把登录材料、消息正文或旧同步计划写入意图。

use traesync_domain::HandoffIntent;

/// 固定恢复区中的承接意图存储。
pub trait HandoffIntentStorePort: Send + Sync {
    fn load_intent(&self) -> Result<Option<HandoffIntent>, String>;
    fn publish_intent(&self, intent: &HandoffIntent) -> Result<(), String>;
}
