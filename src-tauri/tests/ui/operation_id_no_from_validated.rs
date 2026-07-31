// R1 第四次修复：OperationId 不应保留公开的 from_validated 字符串注入入口。
// T01 没有从持久化值恢复 OperationId 的真实需求，移除该入口，
// 防止随机 hex key、认证正文、恢复短语等任意字母数字字符串进入 operation_id。
//
// 此文件应编译失败：from_validated 方法不应存在。

use traesync_domain::OperationId;

fn main() {
    // 随机 hex key 加 op- 前缀——不应被接受
    let _ = OperationId::from_validated("op-a1b2c3d4e5f6789012345abcdef");
}
