// 防止 Windows release 构建时弹出命令行窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// 二进制入口：仅调用库的 run 函数，保持二进制壳薄。
fn main() {
    trae_sync_lib::run();
}
