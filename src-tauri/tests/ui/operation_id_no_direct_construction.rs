// R1：OperationId 内部 String 字段私有，外部无法直接构造。
// 此文件应编译失败：无法访问私有字段。

use traesync_domain::OperationId;

fn main() {
    // 直接构造应失败——字段私有
    let _ = OperationId("op-evil-secret".to_string());
}
