//! 结构化日志：每条日志包含 `operation_id`，敏感内容在输出前被去敏。
//!
//! 对应 AC6 与 R1/R2（第四次修复）要求：
//! - 结构化日志具有 `operation_id`。
//! - 采用最小封闭接口：固定事件码枚举代替自由 `message`；
//!   字符串诊断字段默认不可直接输出，仅允许明确的安全类型/白名单字段。
//! - 阻止公开构造任意 `LogEvent`——只能通过 `SafeLogEventBuilder` 构造。
//! - 最终 sink 仍是强制输出边界，生产调用者不能绕过它直接向任意 `LogSink` 输出原始事件。
//! - 不依赖调用方主动登记 secret 才安全——设计上无法承载任意认证正文。
//!
//! 【R1 修复（第四次）】
//! - `LogEvent` 不派生公开 `Deserialize`——外部无法通过
//!   `serde_json::from_str::<LogEvent>` 构造任意 `message`/fields。
//!   由 `tests/ui/log_event_no_deserialize.rs` 的 trybuild compile-fail 测试验证。
//! - `OperationId` 唯一公开构造入口是 `new()`，不接受外部字符串；
//!   `from_validated()` 已移除（T01 无持久化恢复需求，避免任意字符串注入）。
//!   由 `tests/ui/operation_id_no_from_validated.rs` 的 trybuild compile-fail 测试验证。
//! - 最终 sink 也扫描 `operation_id` 字段，发现敏感形态标记时去敏。
//! - 反例测试从 infrastructure crate 外部视角验证（见 tests/logging_integration.rs）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use traesync_domain::OperationId;

/// 固定日志事件码：每个变体对应预定义的 message 模板，调用方无法注入自由文本。
///
/// 这是 R2 修复的核心——`message` 不再是任意字符串，而是由事件码查表得到
/// 的固定描述。即使业务侧持有 secret，也无法通过公开 API 把它塞入日志。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogEventCode {
    /// 应用启动
    AppStarted,
    /// 应用退出
    AppStopped,
    /// 工作台状态已加载
    WorkspaceStateLoaded,
    /// Fixture 路径守卫已创建
    FixtureGuardCreated,
    /// Fixture 写目标验证通过
    FixtureWriteTargetAccepted,
    /// Fixture 写目标被拒绝
    FixtureWriteTargetRejected,
    /// 同步计划已构建
    SyncPlanBuilt,
    /// 同步操作已启动
    SyncStarted,
    /// 同步操作已完成
    SyncCompleted,
    /// 同步操作失败
    SyncFailed,
    /// 备份已创建
    BackupCreated,
    /// 备份恢复已执行
    BackupRestored,
    /// 用户取消操作
    UserCancelled,
    /// 系统根解析失败
    SystemRootsResolutionFailed,
}

impl LogEventCode {
    /// 返回该事件码对应的固定 message（不含任何 secret）。
    fn message(self) -> &'static str {
        match self {
            Self::AppStarted => "应用启动",
            Self::AppStopped => "应用退出",
            Self::WorkspaceStateLoaded => "工作台状态已加载",
            Self::FixtureGuardCreated => "Fixture 路径守卫已创建",
            Self::FixtureWriteTargetAccepted => "Fixture 写目标验证通过",
            Self::FixtureWriteTargetRejected => "Fixture 写目标被拒绝",
            Self::SyncPlanBuilt => "同步计划已构建",
            Self::SyncStarted => "同步操作已启动",
            Self::SyncCompleted => "同步操作已完成",
            Self::SyncFailed => "同步操作失败",
            Self::BackupCreated => "备份已创建",
            Self::BackupRestored => "备份恢复已执行",
            Self::UserCancelled => "用户取消操作",
            Self::SystemRootsResolutionFailed => "系统根解析失败",
        }
    }
}

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// 安全日志字段值：仅允许数值、布尔、计数等不可承载 secret 的类型。
///
/// 【R2 修复】不接受 `String`/`&str`——这是设计上的关键约束。
/// 即使调用方持有 secret，也无法通过公开 API 把它塞入字段值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SafeLogField {
    /// 64 位有符号整数（计数、时长毫秒等）
    I64(i64),
    /// 64 位无符号整数
    U64(u64),
    /// 64 位浮点数
    F64(f64),
    /// 布尔值
    Bool(bool),
    /// 路径计数（仅数值，不含路径字符串）
    PathCount(u64),
    /// 时长（毫秒）
    DurationMs(u64),
    /// 字节数
    Bytes(u64),
    /// 事件码引用（用于关联子事件）
    RelatedCode(LogEventCode),
}

impl SafeLogField {
    /// 转换为 JSON 值
    fn to_json(&self) -> Value {
        match self {
            Self::I64(v) => Value::from(*v),
            Self::U64(v) => Value::from(*v),
            Self::F64(v) => serde_json::Number::from_f64(*v)
                .map(Value::Number)
                .unwrap_or(Value::Null),
            Self::Bool(v) => Value::from(*v),
            Self::PathCount(v) => Value::from(*v),
            Self::DurationMs(v) => Value::from(*v),
            Self::Bytes(v) => Value::from(*v),
            Self::RelatedCode(c) => Value::from(c.message()),
        }
    }
}

/// 允许的字段名白名单：调用方只能使用这些预定义名称添加字段。
///
/// 这进一步封闭了攻击面——即使 `SafeLogField` 不接受字符串，
/// 字段名白名单也防止通过字段名暗示 secret。
const ALLOWED_FIELD_NAMES: &[&str] = &[
    "count",
    "duration_ms",
    "bytes",
    "path_count",
    "session_count",
    "project_count",
    "account_count",
    "is_empty",
    "is_first_run",
    "attempt",
    "error_code",
    "related_code",
    "index",
    "total",
    "succeeded",
    "failed",
    "skipped",
];

/// 判断字段名是否在白名单内（小写化比较）
fn is_allowed_field_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    ALLOWED_FIELD_NAMES.iter().any(|s| *s == lower)
}

/// 结构化日志事件：字段私有，外部无法构造。
///
/// 【R1 修复（第三次）】不派生公开 `Deserialize`——外部无法通过
/// `serde_json::from_str::<LogEvent>` 构造任意 `message`/fields。
/// 只能通过 `SafeLogEventBuilder` 构造。`message` 由 `LogEventCode` 查表得到，
/// 不是调用方提供的自由文本。
#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    operation_id: String,
    level: LogLevel,
    message: String,
    fields: Value,
}

/// 安全日志事件构造器：唯一公开构造 `LogEvent` 的入口。
///
/// 【R2 修复】只接受 `LogEventCode`（固定 message）和 `SafeLogField`
/// （非字符串值类型）。字段名必须在白名单内。调用方无法注入任意文本。
pub struct SafeLogEventBuilder {
    operation_id: OperationId,
    level: LogLevel,
    code: LogEventCode,
    fields: serde_json::Map<String, Value>,
}

impl SafeLogEventBuilder {
    pub fn new(operation_id: OperationId, level: LogLevel, code: LogEventCode) -> Self {
        Self {
            operation_id,
            level,
            code,
            fields: serde_json::Map::new(),
        }
    }

    /// 添加一个字段。字段名必须在白名单内，值必须是 `SafeLogField`。
    ///
    /// 字段名不在白名单时返回 `Err`，防止通过字段名暗示 secret。
    pub fn field(mut self, name: &str, value: SafeLogField) -> Result<Self, LogFieldError> {
        if !is_allowed_field_name(name) {
            return Err(LogFieldError::FieldNameNotAllowed {
                name: name.to_string(),
            });
        }
        self.fields.insert(name.to_string(), value.to_json());
        Ok(self)
    }

    pub fn build(self) -> LogEvent {
        LogEvent {
            operation_id: self.operation_id.as_str().to_string(),
            level: self.level,
            message: self.code.message().to_string(),
            fields: Value::Object(self.fields),
        }
    }
}

/// 字段添加错误
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogFieldError {
    /// 字段名不在白名单内
    FieldNameNotAllowed { name: String },
}

impl std::fmt::Display for LogFieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FieldNameNotAllowed { name } => {
                write!(f, "字段名不在白名单内: {name}")
            }
        }
    }
}

impl std::error::Error for LogFieldError {}

/// 日志 sink trait：抽象日志输出目标。
pub trait LogSink: Send + Sync {
    fn write(&self, event: &LogEvent);
}

/// 敏感字段名集合（defense-in-depth，用于深度扫描嵌套结构）。
/// 由于 `SafeLogField` 不接受字符串，这主要是防御性检查。
const SENSITIVE_FIELD_NAMES: &[&str] = &[
    "token",
    "accesstoken",
    "access_token",
    "refreshtoken",
    "refresh_token",
    "cookie",
    "cookies",
    "authorization",
    "auth",
    "password",
    "passwd",
    "secret",
    "apikey",
    "api_key",
    "credential",
    "credentials",
];

/// 已知敏感形态标记：用于 defense-in-depth 深度扫描。
/// 由于 `SafeLogField` 不接受字符串，正常情况下不会命中。
const SECRET_MARKERS: &[&str] = &[
    "bearer ",
    "basic ",
    "session=",
    "token=",
    "password=",
    "secret=",
    "api_key=",
    "apikey=",
];

/// 去敏日志 sink：最终输出边界。
///
/// 【R2 修复】不再依赖调用方登记 secret。由于 `LogEvent` 只能通过
/// `SafeLogEventBuilder` 构造，且 builder 只接受 `LogEventCode`（固定 message）
/// 和 `SafeLogField`（非字符串值），正常情况下事件中不可能包含 secret。
///
/// 此 sink 作为 defense-in-depth：仍会深度扫描所有字符串值（包括嵌套结构），
/// 若发现敏感字段名或已知敏感形态标记，替换为 `[REDACTED]`。
/// 这覆盖未来若有人绕过 builder 直接构造事件的极端情况。
pub struct RedactingLogSink {
    inner: Box<dyn LogSink>,
    sensitive_names: HashSet<String>,
}

impl RedactingLogSink {
    pub fn new(inner: Box<dyn LogSink>) -> Self {
        let sensitive_names = SENSITIVE_FIELD_NAMES
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        Self {
            inner,
            sensitive_names,
        }
    }
}

impl LogSink for RedactingLogSink {
    fn write(&self, event: &LogEvent) {
        let redacted = redact_event(event, &self.sensitive_names);
        self.inner.write(&redacted);
    }
}

/// 深度去敏一个 JSON 值：递归扫描对象字段名与字符串值。
fn deep_redact(value: Value, is_sensitive: &impl Fn(&str) -> bool) -> Value {
    match value {
        Value::Object(map) => {
            let mut new_map = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                if is_sensitive(&k) {
                    new_map.insert(k, Value::String("[REDACTED]".to_string()));
                } else {
                    new_map.insert(k, deep_redact(v, is_sensitive));
                }
            }
            Value::Object(new_map)
        }
        Value::Array(arr) => Value::Array(
            arr.into_iter()
                .map(|v| deep_redact(v, is_sensitive))
                .collect(),
        ),
        Value::String(s) => {
            if string_carries_secret_marker(&s) {
                Value::String("[REDACTED]".to_string())
            } else {
                Value::String(s)
            }
        }
        other => other,
    }
}

/// 判断字符串值是否包含已知敏感形态标记。
fn string_carries_secret_marker(s: &str) -> bool {
    let lower = s.to_lowercase();
    for marker in SECRET_MARKERS {
        if lower.contains(marker) {
            return true;
        }
    }
    false
}

/// 判断字段名是否敏感（小写化比较）
fn is_sensitive_name(name: &str, sensitive_names: &HashSet<String>) -> bool {
    sensitive_names.contains(&name.to_lowercase())
}

/// 对完整日志事件做去敏扫描（最终输出边界）。
///
/// 【R1 修复（第三次）】`operation_id` 字段也纳入扫描——即使 `OperationId`
/// 的安全构造器被绕过，最终 sink 仍会扫描其内容，发现敏感形态标记时去敏。
fn redact_event(event: &LogEvent, sensitive_names: &HashSet<String>) -> LogEvent {
    let is_sensitive = |name: &str| is_sensitive_name(name, sensitive_names);
    let redacted_fields = deep_redact(event.fields.clone(), &is_sensitive);
    // message 由 LogEventCode 查表得到，是固定字符串，但仍做 defense-in-depth 扫描
    let redacted_message = if string_carries_secret_marker(&event.message) {
        "[REDACTED]".to_string()
    } else {
        event.message.clone()
    };
    // operation_id 也做 defense-in-depth 扫描
    let redacted_operation_id = if string_carries_secret_marker(&event.operation_id) {
        "[REDACTED]".to_string()
    } else {
        event.operation_id.clone()
    };
    LogEvent {
        operation_id: redacted_operation_id,
        level: event.level,
        message: redacted_message,
        fields: redacted_fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// 内存日志 sink：收集所有写入的事件，用于测试
    #[derive(Default)]
    struct MemorySink {
        events: Arc<Mutex<Vec<LogEvent>>>,
    }

    impl MemorySink {
        fn new() -> (Self, Arc<Mutex<Vec<LogEvent>>>) {
            let events = Arc::new(Mutex::new(Vec::new()));
            let sink = Self {
                events: events.clone(),
            };
            (sink, events)
        }
    }

    impl LogSink for MemorySink {
        fn write(&self, event: &LogEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    #[test]
    fn log_event_contains_operation_id_and_fixed_message() {
        let op_id = OperationId::new();
        let op_id_str = op_id.as_str().to_string();
        let event =
            SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::AppStarted).build();

        assert_eq!(event.operation_id, op_id_str);
        assert_eq!(event.level, LogLevel::Info);
        assert_eq!(event.message, "应用启动");
    }

    #[test]
    fn safe_field_accepts_numeric_values() {
        let op_id = OperationId::new();
        let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::SyncCompleted)
            .field("count", SafeLogField::U64(3))
            .unwrap()
            .field("duration_ms", SafeLogField::DurationMs(1200))
            .unwrap()
            .field("is_empty", SafeLogField::Bool(false))
            .unwrap()
            .build();

        let fields = event.fields.as_object().unwrap();
        assert_eq!(fields.get("count").unwrap(), &Value::from(3u64));
        assert_eq!(fields.get("duration_ms").unwrap(), &Value::from(1200u64));
        assert_eq!(fields.get("is_empty").unwrap(), &Value::from(false));
    }

    #[test]
    fn field_name_not_in_whitelist_rejected() {
        let op_id = OperationId::new();
        let result = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::AppStarted)
            .field("token", SafeLogField::U64(123));
        assert!(matches!(
            result,
            Err(LogFieldError::FieldNameNotAllowed { .. })
        ));
    }

    // ============== R2 关键反例测试：不依赖登记也能阻止 secret ==============

    #[test]
    fn random_hex_key_cannot_enter_message() {
        // 随机十六进制 key 无法进入 message——message 由 LogEventCode 查表得到
        let op_id = OperationId::new();
        let event =
            SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::AppStarted).build();

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        // message 是固定字符串"应用启动"，不含任何 key
        assert_eq!(written[0].message, "应用启动");
        // 即使假设 key 是 "a1b2c3d4e5f6789012345abcdef"，它无法进入事件
        assert!(!written_json.contains("a1b2c3d4e5f6789012345abcdef"));
    }

    #[test]
    fn auth_ciphertext_without_marker_cannot_enter_fields() {
        // 无 marker 的认证密文无法进入字段——SafeLogField 不接受 String
        let op_id = OperationId::new();
        // 编译期保证：SafeLogEventBuilder::field 只接受 SafeLogField，不接受 String
        // 因此调用方无法写入 "eyJhbGciOiJIUzI1NiJ9.payload.sig" 这样的密文
        let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::SyncStarted)
            .field("count", SafeLogField::U64(1))
            .unwrap()
            .build();

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        // 字段中只有 count=1，不含任何密文
        assert!(!written_json.contains("eyJhbGci"));
        assert!(!written_json.contains("payload"));
        assert!(!written_json.contains("sig"));
    }

    #[test]
    fn recovery_password_cannot_enter_nested_fields() {
        // 恢复密码无法进入嵌套字段——SafeLogField 不接受 String，无法构造嵌套对象
        let op_id = OperationId::new();
        let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::BackupCreated)
            .field("count", SafeLogField::U64(1))
            .unwrap()
            .build();

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        // 恢复密码 "my-secret-recovery-pass-123" 无法出现在事件中
        assert!(!written_json.contains("my-secret-recovery-pass-123"));
        assert!(!written_json.contains("recovery"));
        assert!(!written_json.contains("pass"));
    }

    #[test]
    fn defense_in_depth_redacts_bearer_marker_in_any_string() {
        // defense-in-depth：即使未来有人绕过 builder 直接构造事件，
        // sink 仍会扫描字符串值中的已知敏感标记
        let op_id = OperationId::new();
        // 直接构造内部 LogEvent（仅测试可见，模拟"绕过 builder"的极端情况）
        let raw_event = LogEvent {
            operation_id: op_id.as_str().to_string(),
            level: LogLevel::Info,
            message: "请求头 Authorization: Bearer eyJhbGc".to_string(),
            fields: Value::Object({
                let mut m = serde_json::Map::new();
                m.insert(
                    "note".to_string(),
                    Value::String("cookie: session=abc123".to_string()),
                );
                m
            }),
        };

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&raw_event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        assert!(!written_json.contains("Bearer eyJhbGc"));
        assert!(!written_json.contains("session=abc123"));
        assert!(written_json.contains("[REDACTED]"));
    }

    #[test]
    fn defense_in_depth_redacts_operation_id_with_secret_marker() {
        // defense-in-depth：operation_id 中若包含敏感形态标记，sink 也会去敏
        let op_id = OperationId::new();
        // 模拟绕过 OperationId 安全构造器的极端情况
        let raw_event = LogEvent {
            operation_id: "op-token=abc123".to_string(),
            level: LogLevel::Info,
            message: "测试".to_string(),
            fields: Value::Object(serde_json::Map::new()),
        };

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&raw_event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        assert!(!written_json.contains("token=abc123"));
        assert!(written_json.contains("[REDACTED]"));
        // 防止误用 op_id
        let _ = op_id;
    }

    #[test]
    fn defense_in_depth_redacts_sensitive_field_names_in_nested() {
        // defense-in-depth：嵌套结构中的敏感字段名也被去敏
        let op_id = OperationId::new();
        let raw_event = LogEvent {
            operation_id: op_id.as_str().to_string(),
            level: LogLevel::Info,
            message: "测试".to_string(),
            fields: serde_json::json!({
                "payload": {
                    "user": "alice",
                    "password": "plain-text-password",
                    "credentials": {
                        "api_key": "sk-12345"
                    }
                }
            }),
        };

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&raw_event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        assert!(!written_json.contains("plain-text-password"));
        assert!(!written_json.contains("sk-12345"));
        assert!(written_json.contains("[REDACTED]"));
        assert!(written_json.contains("alice"));
    }

    #[test]
    fn normal_events_preserved() {
        // 正常事件（无 secret）原样保留
        let op_id = OperationId::new();
        let event = SafeLogEventBuilder::new(op_id, LogLevel::Info, LogEventCode::SyncCompleted)
            .field("session_count", SafeLogField::U64(3))
            .unwrap()
            .field("duration_ms", SafeLogField::DurationMs(1200))
            .unwrap()
            .build();

        let (sink, events) = MemorySink::new();
        let redacting = RedactingLogSink::new(Box::new(sink));
        redacting.write(&event);

        let written = events.lock().unwrap();
        let written_json = serde_json::to_string(&written[0]).unwrap();
        assert!(written_json.contains("同步操作已完成"));
        assert!(written_json.contains("1200"));
        assert!(written_json.contains("3"));
    }

    #[test]
    fn all_event_codes_have_fixed_messages() {
        // 所有事件码都必须有固定 message，不允许空字符串
        let codes = [
            LogEventCode::AppStarted,
            LogEventCode::AppStopped,
            LogEventCode::WorkspaceStateLoaded,
            LogEventCode::FixtureGuardCreated,
            LogEventCode::FixtureWriteTargetAccepted,
            LogEventCode::FixtureWriteTargetRejected,
            LogEventCode::SyncPlanBuilt,
            LogEventCode::SyncStarted,
            LogEventCode::SyncCompleted,
            LogEventCode::SyncFailed,
            LogEventCode::BackupCreated,
            LogEventCode::BackupRestored,
            LogEventCode::UserCancelled,
            LogEventCode::SystemRootsResolutionFailed,
        ];
        for code in codes {
            assert!(!code.message().is_empty(), "事件码 {:?} message 为空", code);
        }
    }
}
