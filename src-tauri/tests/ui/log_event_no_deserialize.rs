// R1：LogEvent 不实现 Deserialize，外部无法通过 serde 反序列化构造任意事件。
// 此文件应编译失败：LogEvent 未实现 Deserialize trait。

use traesync_infrastructure::LogEvent;

fn main() {
    let json = r#"{"operation_id":"evil","message":"secret","fields":{}}"#;
    let _event: LogEvent = serde_json::from_str(json).unwrap();
}
