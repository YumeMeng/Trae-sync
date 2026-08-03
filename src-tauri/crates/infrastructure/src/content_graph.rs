//! 确定性内容图哈希器：Gate I 核心。
//!
//! 实现 `ContentGraphHasher` port，提供会话内容图哈希与版本分类。
//!
//! 确定性要求（Gate I）：
//! - 消息按稳定 message_id 排序后哈希，不依赖 SQLite 行顺序
//! - 只使用语义字段（message_id + role + content_excerpt + seq），
//!   不使用时间戳、缓存、FTS、派生字段
//! - 哈希输出 SHA-256 hex
//!
//! 版本分类：
//! - Identical: 新旧消息集合完全相同（按 message_id + 内容哈希）
//! - FastForward: 旧消息全部保留且内容不变，新消息只追加
//! - Forked: 旧消息被修改/删除/重排，或双方各有新增
//! - Unclassified: 缺少稳定 ID 或无法判定

use sha2::{Digest, Sha256};
use traesync_domain::{
    ContentGraphHash, MessageProjection, SessionIdentity, VersionClassification,
};
use traesync_ports::ContentGraphHasher;

/// R4：规范化 JSON 内容——解析后递归排序 key 再序列化。
/// 确保 `{"a":1,"b":2}` 与 `{"b":2,"a":1}` 产生相同哈希。
/// 非 JSON 内容原样返回。
fn canonicalize_json(content: &str) -> String {
    let trimmed = content.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return content.to_string();
    }
    // 尝试解析为 serde_json::Value 并规范化
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(value) => {
            let canonical = canonicalize_value(&value);
            serde_json::to_string(&canonical).unwrap_or_else(|_| content.to_string())
        }
        Err(_) => content.to_string(),
    }
}

/// 递归规范化 JSON Value：对象 key 排序，数组保持顺序。
fn canonicalize_value(value: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    use std::collections::BTreeMap;
    match value {
        Value::Object(map) => {
            // BTreeMap 自动按 key 排序
            let sorted: BTreeMap<&str, &Value> = map.iter().map(|(k, v)| (k.as_str(), v)).collect();
            let mut result = serde_json::Map::new();
            for (k, v) in sorted {
                result.insert(k.to_string(), canonicalize_value(v));
            }
            Value::Object(result)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(canonicalize_value).collect()),
        other => other.clone(),
    }
}

/// 确定性内容图哈希器。
pub struct DeterministicContentGraphHasher;

impl Default for DeterministicContentGraphHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl DeterministicContentGraphHasher {
    pub fn new() -> Self {
        Self
    }
}

impl ContentGraphHasher for DeterministicContentGraphHasher {
    fn hash_session_content(
        &self,
        messages: &[MessageProjection],
        _session: &SessionIdentity,
    ) -> ContentGraphHash {
        // 1. 过滤软删除消息（软删除不进入内容图哈希）
        let visible: Vec<&MessageProjection> =
            messages.iter().filter(|m| !m.soft_deleted).collect();

        // 2. 按 message_id 排序（稳定 ID，不依赖行顺序）
        let mut sorted: Vec<&MessageProjection> = visible;
        sorted.sort_by(|a, b| a.message_id.cmp(&b.message_id));

        // 3. R4：哈希——每条消息用 message_id|role|seq|canonical_content|turn_id 拼接
        //    - content 经 JSON 规范化，消除 key 顺序影响
        //    - turn_id 参与哈希，确保 turn 关系变化影响版本分类
        let mut hasher = Sha256::new();
        for m in &sorted {
            hasher.update(m.message_id.as_bytes());
            hasher.update(b"|");
            hasher.update(m.role.as_bytes());
            hasher.update(b"|");
            hasher.update(m.seq.to_le_bytes());
            hasher.update(b"|");
            let canonical = canonicalize_json(&m.content_excerpt);
            hasher.update(canonical.as_bytes());
            hasher.update(b"|");
            // R4：turn_id 参与哈希——None 和 Some("x") 产生不同字节
            match &m.turn_id {
                Some(tid) => {
                    hasher.update(b"turn:");
                    hasher.update(tid.as_bytes());
                }
                None => {
                    hasher.update(b"turn:none");
                }
            }
            hasher.update(b"\n");
        }
        ContentGraphHash(hex::encode(hasher.finalize()))
    }

    fn classify(
        &self,
        old_messages: &[MessageProjection],
        new_messages: &[MessageProjection],
    ) -> VersionClassification {
        // 1. 过滤软删除
        let old_visible: Vec<&MessageProjection> =
            old_messages.iter().filter(|m| !m.soft_deleted).collect();
        let new_visible: Vec<&MessageProjection> =
            new_messages.iter().filter(|m| !m.soft_deleted).collect();

        // 2. 检查是否有稳定 ID（message_id 非空）
        if old_visible.iter().any(|m| m.message_id.is_empty())
            || new_visible.iter().any(|m| m.message_id.is_empty())
        {
            return VersionClassification::Unclassified;
        }

        // 3. R4：构建旧消息映射：message_id -> (role, seq, canonical_content, turn_id)
        //    使用规范化后的 content 进行比较，消除 JSON key 顺序影响
        use std::collections::HashMap;
        let old_map: HashMap<&str, (&str, u64, String, Option<&str>)> = old_visible
            .iter()
            .map(|m| {
                (
                    m.message_id.as_str(),
                    (
                        m.role.as_str(),
                        m.seq,
                        canonicalize_json(&m.content_excerpt),
                        m.turn_id.as_deref(),
                    ),
                )
            })
            .collect();

        // 4. 构建新消息映射
        let new_map: HashMap<&str, (&str, u64, String, Option<&str>)> = new_visible
            .iter()
            .map(|m| {
                (
                    m.message_id.as_str(),
                    (
                        m.role.as_str(),
                        m.seq,
                        canonicalize_json(&m.content_excerpt),
                        m.turn_id.as_deref(),
                    ),
                )
            })
            .collect();

        // 5. 如果两个映射完全相同 -> Identical
        //    注意：HashMap 比较不依赖顺序，满足行序不变性
        if old_map == new_map {
            return VersionClassification::Identical;
        }

        // 6. 检查 FastForward：旧消息全部保留且内容不变，新消息只追加
        let mut old_all_preserved = true;
        let mut old_content_unchanged = true;
        for (mid, (role, seq, content, turn_id)) in &old_map {
            match new_map.get(*mid) {
                None => {
                    // 旧消息被删除 -> 不是 FastForward
                    old_all_preserved = false;
                }
                Some((new_role, new_seq, new_content, new_turn_id)) => {
                    // R4：内容、角色或 turn_id 变化 -> 不是 FastForward
                    if role != new_role || content != new_content || turn_id != new_turn_id {
                        old_content_unchanged = false;
                    }
                    // seq 变化视为重排 -> 不是 FastForward
                    if seq != new_seq {
                        old_content_unchanged = false;
                    }
                }
            }
        }

        if old_all_preserved && old_content_unchanged {
            // 旧消息全部保留且内容不变，且有新增消息
            let has_new =
                new_map.len() > old_map.len() || new_map.keys().any(|k| !old_map.contains_key(k));
            if has_new {
                return VersionClassification::FastForward;
            }
            // 旧消息全部保留且内容不变，但无新增——且不 Identical（seq 不同等情况已排除）
            // 理论上不会到这里，保守返回 Identical
            return VersionClassification::Identical;
        }

        // 7. 其他情况 -> Forked（修改、删除、重排、双分支）
        VersionClassification::Forked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(mid: &str, role: &str, content: &str, seq: u64) -> MessageProjection {
        MessageProjection {
            message_id: mid.to_string(),
            session_id: "s1".to_string(),
            role: role.to_string(),
            content_excerpt: content.to_string(),
            soft_deleted: false,
            seq,
            turn_id: None,
        }
    }

    /// R4：带 turn_id 的消息构造辅助
    fn msg_with_turn(
        mid: &str,
        role: &str,
        content: &str,
        seq: u64,
        turn_id: &str,
    ) -> MessageProjection {
        MessageProjection {
            message_id: mid.to_string(),
            session_id: "s1".to_string(),
            role: role.to_string(),
            content_excerpt: content.to_string(),
            soft_deleted: false,
            seq,
            turn_id: Some(turn_id.to_string()),
        }
    }

    fn session() -> SessionIdentity {
        SessionIdentity::new("work_cn", "sess-001")
    }

    #[test]
    fn hash_deterministic_regardless_of_input_order() {
        // Gate I：行顺序不影响哈希
        let msgs1 = vec![
            msg("m1", "user", "hello", 1),
            msg("m2", "assistant", "hi", 2),
        ];
        let msgs2 = vec![
            msg("m2", "assistant", "hi", 2),
            msg("m1", "user", "hello", 1),
        ];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs1, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs2, &session());
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_excludes_soft_deleted() {
        // 软删除消息不进入内容图哈希
        let mut soft = msg("m1", "user", "hello", 1);
        soft.soft_deleted = true;
        let msgs_with_soft = vec![soft, msg("m2", "assistant", "hi", 2)];
        let msgs_without = vec![msg("m2", "assistant", "hi", 2)];
        let h1 = DeterministicContentGraphHasher::new()
            .hash_session_content(&msgs_with_soft, &session());
        let h2 =
            DeterministicContentGraphHasher::new().hash_session_content(&msgs_without, &session());
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_changes_with_content_change() {
        let msgs1 = vec![msg("m1", "user", "hello", 1)];
        let msgs2 = vec![msg("m1", "user", "world", 1)];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs1, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs2, &session());
        assert_ne!(h1, h2);
    }

    #[test]
    fn classify_identical() {
        let old = vec![
            msg("m1", "user", "hello", 1),
            msg("m2", "assistant", "hi", 2),
        ];
        let new = vec![
            msg("m2", "assistant", "hi", 2),
            msg("m1", "user", "hello", 1),
        ];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Identical);
    }

    #[test]
    fn classify_fast_forward() {
        // 旧消息全部保留且内容不变，新消息只追加
        let old = vec![msg("m1", "user", "hello", 1)];
        let new = vec![
            msg("m1", "user", "hello", 1),
            msg("m2", "assistant", "hi", 2),
        ];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::FastForward);
    }

    #[test]
    fn classify_forked_on_content_modification() {
        // 既有内容修改 -> Forked
        let old = vec![msg("m1", "user", "hello", 1)];
        let new = vec![msg("m1", "user", "world", 1)];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Forked);
    }

    #[test]
    fn classify_forked_on_deletion() {
        // 旧消息被删除 -> Forked
        let old = vec![
            msg("m1", "user", "hello", 1),
            msg("m2", "assistant", "hi", 2),
        ];
        let new = vec![msg("m1", "user", "hello", 1)];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Forked);
    }

    #[test]
    fn classify_forked_on_reorder() {
        // seq 变化（重排）-> Forked
        let old = vec![
            msg("m1", "user", "hello", 1),
            msg("m2", "assistant", "hi", 2),
        ];
        let new = vec![
            msg("m1", "user", "hello", 2),
            msg("m2", "assistant", "hi", 1),
        ];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Forked);
    }

    #[test]
    fn classify_forked_on_two_sided_additions() {
        // 双分支：双方各有新增
        let old = vec![
            msg("m1", "user", "hello", 1),
            msg("m_old", "user", "old-only", 3),
        ];
        let new = vec![
            msg("m1", "user", "hello", 1),
            msg("m_new", "user", "new-only", 3),
        ];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Forked);
    }

    #[test]
    fn classify_unclassified_on_missing_stable_id() {
        // 缺少稳定 ID -> Unclassified
        let old = vec![msg("", "user", "hello", 1)];
        let new = vec![msg("m1", "user", "hello", 1)];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Unclassified);
    }

    #[test]
    fn classify_forked_on_role_change() {
        // 角色变化 -> Forked
        let old = vec![msg("m1", "user", "hello", 1)];
        let new = vec![msg("m1", "assistant", "hello", 1)];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(c, VersionClassification::Forked);
    }

    // ============== R4：完整与规范内容图反例测试 ==============

    #[test]
    fn r4_long_content_suffix_change_affects_hash() {
        // 反例：第 500 字符之后的变化必须影响哈希
        let prefix: String = "a".repeat(500);
        let content_a = format!("{}suffix_a", prefix);
        let content_b = format!("{}suffix_b", prefix);
        let msgs_a = vec![msg("m1", "user", &content_a, 1)];
        let msgs_b = vec![msg("m1", "user", &content_b, 1)];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_a, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_b, &session());
        assert_ne!(h1, h2, "第 500 字符后的变化必须影响哈希");
    }

    #[test]
    fn r4_long_content_suffix_change_affects_classification() {
        // 反例：第 500 字符后的变化必须导致 Forked（而非 Identical）
        let prefix: String = "a".repeat(600);
        let old = vec![msg("m1", "user", &format!("{}X", prefix), 1)];
        let new = vec![msg("m1", "user", &format!("{}Y", prefix), 1)];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(
            c,
            VersionClassification::Forked,
            "第 500 字符后的变化应导致 Forked"
        );
    }

    #[test]
    fn r4_json_key_order_invariance_hash() {
        // 反例：JSON key 顺序不同不应影响哈希
        let content_a = r#"{"name":"alice","age":30,"city":"NYC"}"#;
        let content_b = r#"{"city":"NYC","age":30,"name":"alice"}"#;
        let msgs_a = vec![msg("m1", "user", content_a, 1)];
        let msgs_b = vec![msg("m1", "user", content_b, 1)];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_a, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_b, &session());
        assert_eq!(h1, h2, "JSON key 顺序不同应产生相同哈希");
    }

    #[test]
    fn r4_json_key_order_invariance_classification() {
        // JSON key 顺序不同应判 Identical
        let content_a = r#"{"name":"alice","age":30}"#;
        let content_b = r#"{"age":30,"name":"alice"}"#;
        let old = vec![msg("m1", "user", content_a, 1)];
        let new = vec![msg("m1", "user", content_b, 1)];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(
            c,
            VersionClassification::Identical,
            "JSON key 顺序不同应判 Identical"
        );
    }

    #[test]
    fn r4_nested_json_key_order_invariance() {
        // 嵌套 JSON key 顺序不同也不应影响哈希
        let content_a = r#"{"outer":{"z":1,"a":2},"list":[1,2,3]}"#;
        let content_b = r#"{"list":[1,2,3],"outer":{"a":2,"z":1}}"#;
        let msgs_a = vec![msg("m1", "user", content_a, 1)];
        let msgs_b = vec![msg("m1", "user", content_b, 1)];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_a, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_b, &session());
        assert_eq!(h1, h2, "嵌套 JSON key 顺序不同应产生相同哈希");
    }

    #[test]
    fn r4_json_value_change_affects_hash() {
        // JSON 值变化应影响哈希
        let content_a = r#"{"name":"alice"}"#;
        let content_b = r#"{"name":"bob"}"#;
        let msgs_a = vec![msg("m1", "user", content_a, 1)];
        let msgs_b = vec![msg("m1", "user", content_b, 1)];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_a, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_b, &session());
        assert_ne!(h1, h2, "JSON 值变化应影响哈希");
    }

    #[test]
    fn r4_non_json_content_not_affected_by_canonicalization() {
        // 非 JSON 内容不受规范化影响——原样参与哈希
        let content = "Hello, world! This is plain text.";
        let msgs1 = vec![msg("m1", "user", content, 1)];
        let msgs2 = vec![msg("m1", "user", content, 1)];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs1, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs2, &session());
        assert_eq!(h1, h2);
    }

    #[test]
    fn r4_row_order_invariance_with_turn_id() {
        // 带 turn_id 的消息行序不变性
        let msgs1 = vec![
            msg_with_turn("m1", "user", "hello", 1, "t1"),
            msg_with_turn("m2", "assistant", "hi", 2, "t1"),
        ];
        let msgs2 = vec![
            msg_with_turn("m2", "assistant", "hi", 2, "t1"),
            msg_with_turn("m1", "user", "hello", 1, "t1"),
        ];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs1, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs2, &session());
        assert_eq!(h1, h2, "行序不影响哈希（含 turn_id）");
    }

    #[test]
    fn r4_turn_id_change_affects_hash() {
        // turn_id 变化应影响哈希
        let msgs_a = vec![msg_with_turn("m1", "user", "hello", 1, "t1")];
        let msgs_b = vec![msg_with_turn("m1", "user", "hello", 1, "t2")];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_a, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_b, &session());
        assert_ne!(h1, h2, "turn_id 变化应影响哈希");
    }

    #[test]
    fn r4_turn_id_change_affects_classification() {
        // turn_id 变化应导致 Forked
        let old = vec![msg_with_turn("m1", "user", "hello", 1, "t1")];
        let new = vec![msg_with_turn("m1", "user", "hello", 1, "t2")];
        let c = DeterministicContentGraphHasher::new().classify(&old, &new);
        assert_eq!(
            c,
            VersionClassification::Forked,
            "turn_id 变化应导致 Forked"
        );
    }

    #[test]
    fn r4_turn_id_none_vs_some_affects_hash() {
        // None vs Some("t1") 应产生不同哈希
        let msgs_a = vec![msg("m1", "user", "hello", 1)]; // turn_id = None
        let msgs_b = vec![msg_with_turn("m1", "user", "hello", 1, "t1")];
        let h1 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_a, &session());
        let h2 = DeterministicContentGraphHasher::new().hash_session_content(&msgs_b, &session());
        assert_ne!(h1, h2, "None vs Some(turn_id) 应产生不同哈希");
    }
}
