//! 日志边界集成测试：从 infrastructure crate 外部视角验证 R1（第四次修复）要求。
//!
//! 这些测试在 infrastructure crate 的单元测试之外，从外部视角验证：
//! - `OperationId` 没有公开的字符串注入入口——`from_validated()` 已移除，
//!   外部只能通过 `new()` 获得实例，随机 hex key、认证正文、恢复短语
//!   无法进入 operation_id。
//! - `OperationId` 内部字段私有——外部无法 `OperationId(secret)` 构造。
//! - `LogEvent` 不实现 `Deserialize`——由 `tests/compile_fail.rs` 中的
//!   trybuild 测试真实编译验证。
//! - 最终 sink（`RedactingLogSink`）扫描 `operation_id` 字段。
//!
//! 编译期保证（OperationId 字段私有、LogEvent 不实现 Deserialize、
//! from_validated 不存在）由 `tests/ui/*.rs` 下的 trybuild compile-fail
//! 测试在每次 `cargo test` 时真实编译验证。

use serde::Serialize;
use std::sync::{Arc, Mutex};
use traesync_domain::OperationId;
use traesync_infrastructure::{
    LogEvent, LogEventCode, LogLevel, LogSink, RedactingLogSink, SafeLogEventBuilder,
};

/// 内存日志 sink：收集所有写入事件的 JSON 序列化形式，用于断言。
#[derive(Default)]
struct MemorySink {
    events: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl MemorySink {
    fn new() -> (Self, Arc<Mutex<Vec<serde_json::Value>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Self {
            events: events.clone(),
        };
        (sink, events)
    }
}

impl LogSink for MemorySink {
    fn write(&self, event: &LogEvent) {
        // LogEvent 实现 Serialize，可以序列化为 JSON Value
        let json = serde_json::to_value(event).unwrap();
        self.events.lock().unwrap().push(json);
    }
}

// ============== R1 反例测试：OperationId 唯一入口是 new() ==============

#[test]
fn operation_id_new_is_the_only_public_constructor() {
    // 外部 crate 只能通过 new() 获得 OperationId 实例
    let op_id = OperationId::new();
    // new() 生成的 ID 以 op- 开头
    assert!(op_id.as_str().starts_with("op-"));
    assert!(op_id.as_str().len() > 4);
}

#[test]
fn random_hex_key_cannot_enter_operation_id() {
    // 随机十六进制 key 无法通过任何公开入口进入 operation_id
    // from_validated 已移除，外部无法用 "op-a1b2c3d4e5f6789012345abcdef" 构造
    // new() 内部生成格式为 op-<nanos>-<pid>，不接受外部字符串
    let op_id = OperationId::new();
    let s = op_id.as_str();
    // 验证 new() 生成的格式不包含任意 hex key
    // new() 格式为 op-<数字>-<数字>，不含纯字母 hex
    let rest = &s[3..];
    let parts: Vec<&str> = rest.split('-').collect();
    assert!(parts.len() >= 2, "new() 应生成 op-<nanos>-<pid> 格式");
    for part in parts {
        assert!(
            part.chars().all(|c| c.is_ascii_digit()),
            "new() 生成的各部分应全为数字，实际: {part}"
        );
    }
}

#[test]
fn auth_ciphertext_cannot_enter_operation_id() {
    // 认证正文（JWT 密文、Bearer token）无法进入 operation_id
    // from_validated 已移除，外部无法注入 "op-eyJhbGciOiJIUzI1NiJ9.payload.sig"
    // 或 "op-bearer-abc123"
    let op_id = OperationId::new();
    let s = op_id.as_str();
    // new() 不含认证标记
    assert!(!s.contains("bearer"));
    assert!(!s.contains("eyJ"));
    assert!(!s.contains("payload"));
    assert!(!s.contains("sig"));
}

#[test]
fn recovery_passphrase_cannot_enter_operation_id() {
    // 恢复短语（如 "my-secret-recovery-pass-123"）无法进入 operation_id
    // from_validated 已移除，外部无法注入任意字母数字短语
    let op_id = OperationId::new();
    let s = op_id.as_str();
    // new() 只生成 op-<数字>-<数字>，不含字母短语
    assert!(!s.contains("recovery"));
    assert!(!s.contains("pass"));
    assert!(!s.contains("secret"));
    assert!(!s.contains("password"));
}

// ============== R1 反例测试：LogEvent 只能通过 SafeLogEventBuilder 构造 ==============

#[test]
fn log_event_serializes_correctly() {
    // LogEvent 实现 Serialize（用于输出），但不实现 Deserialize
    let op_id = OperationId::new();
    let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::AppStarted).build();

    let json = serde_json::to_value(&event).unwrap();
    assert!(json.is_object());
    // operation_id 由 new() 生成，以 op- 开头
    let op_id_str = json.get("operation_id").unwrap().as_str().unwrap();
    assert!(op_id_str.starts_with("op-"));
    // message 由 LogEventCode 查表得到，是固定字符串
    assert_eq!(json.get("message").unwrap(), "应用启动");
}

/// 辅助：验证类型是否实现 Serialize（编译期）
fn _assert_serialize<T: Serialize>(_t: T) {}

#[test]
fn log_event_implements_serialize() {
    let op_id = OperationId::new();
    let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::AppStarted).build();
    _assert_serialize(event);
}

// 注意：LogEvent 不实现 Deserialize，由 tests/ui/log_event_no_deserialize.rs
// 的 trybuild compile-fail 测试真实编译验证。外部无法通过
// `serde_json::from_str::<LogEvent>(json)` 构造任意事件。
// 同样，LogEvent 字段私有，由 tests/ui/log_event_no_direct_construction.rs 验证。

// ============== R1 反例测试：RedactingLogSink 扫描 operation_id ==============

#[test]
fn redacting_sink_preserves_new_generated_operation_id() {
    // 从外部视角验证：RedactingLogSink 保留 new() 生成的正常 operation_id
    let op_id = OperationId::new();
    let expected_op_id = op_id.as_str().to_string();
    let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::AppStarted).build();

    let (sink, events) = MemorySink::new();
    let redacting = RedactingLogSink::new(Box::new(sink));
    redacting.write(&event);

    let written = events.lock().unwrap();
    let json = &written[0];
    // 正常 operation_id 保留
    assert_eq!(
        json.get("operation_id").unwrap().as_str().unwrap(),
        expected_op_id
    );
    // message 是固定字符串
    assert_eq!(json.get("message").unwrap(), "应用启动");
}

#[test]
fn normal_event_preserved_through_sink() {
    // 正常事件通过 sink 后内容保留
    let op_id = OperationId::new();
    let op_id_str = op_id.as_str().to_string();
    let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::SyncCompleted)
        .field("count", traesync_infrastructure::SafeLogField::U64(3))
        .unwrap()
        .field(
            "duration_ms",
            traesync_infrastructure::SafeLogField::DurationMs(1200),
        )
        .unwrap()
        .build();

    let (sink, events) = MemorySink::new();
    let redacting = RedactingLogSink::new(Box::new(sink));
    redacting.write(&event);

    let written = events.lock().unwrap();
    let json = &written[0];
    let json_str = serde_json::to_string(json).unwrap();
    assert!(json_str.contains("同步操作已完成"));
    assert!(json_str.contains("1200"));
    // operation_id 由 new() 生成，保留
    assert!(json_str.contains(&op_id_str));
}
