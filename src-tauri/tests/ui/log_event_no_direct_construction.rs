// R1：LogEvent 字段全部私有，外部无法直接结构体构造。
// 此文件应编译失败：字段私有无法访问。

use serde_json::Value;
use traesync_infrastructure::{LogEvent, LogLevel};

fn main() {
    let _event = LogEvent {
        operation_id: "evil".to_string(),
        level: LogLevel::Info,
        message: "secret".to_string(),
        fields: Value::Null,
    };
}
