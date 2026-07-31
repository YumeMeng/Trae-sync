//! 编译期反例测试：验证 OperationId 与 LogEvent 的安全边界由类型系统强制。
//!
//! 这些测试使用 `trybuild` 真实编译应失败的源文件，确认外部 crate 无法：
//! - 调用 `OperationId::from_validated`（R1 第四次修复：移除公开字符串注入入口）
//! - 直接构造 `OperationId`（内部字段私有）
//! - 通过 serde 反序列化 `LogEvent`（不实现 Deserialize）
//! - 直接构造 `LogEvent`（字段私有）
//!
//! 与注释掉的代码不同，trybuild 会在每次 `cargo test` 时真实调用 rustc，
//! 任何对安全边界的回退都会导致测试失败。

#[test]
fn compile_fail_boundaries() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}
