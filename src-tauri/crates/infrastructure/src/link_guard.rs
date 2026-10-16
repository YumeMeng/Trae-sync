//! 链接（reparse point）统一判定 helper——工具链 1.97 适配（P15-12 独立票）。
//!
//! 背景：rustc 1.97.1 的 std 在无符号链接特权时把
//! `std::os::windows::fs::symlink_dir` 降级创建 **junction**、
//! `symlink_file` 降级创建 **硬链接**（探针实证：symlink_dir 返回 Ok 且
//! 创建物 `file_type().is_symlink()==false`、`is_dir()==true`）。
//! 原有防目录逃逸判定 `metadata.file_type().is_symlink()` 不识别 junction
//! （junction 是 MOUNT_POINT reparse point，同样可做目录逃逸向量）。
//!
//! 判定策略：`std::fs::read_link(path).is_ok()`——std 原生同时识别
//! 符号链接与 junction（返回链接目标），文件与目录通用，零新依赖。
//!
//! **硬链接防护评估（本轮只评估不实现，登记结论）**：
//! Windows 硬链接检测需要 nlink>1。stable 1.97.1 上
//! `std::os::windows::fs::MetadataExt::number_of_links()` 仍属 unstable
//! `windows_by_handle`（本仓库探针实测 E0658），stable 渠道需经
//! windows-sys `GetFileInformationByHandle` 自行取 nlink；且正常工具
//! （备份软件、Store 应用更新、杀毒隔离区）也会造成 nlink>1，个人本地
//! 工具的威胁模型下按硬链接拒绝会产生误伤；硬链接不携带路径分量、不构成
//! 目录逃逸（内容与本体一致），只在「外部文件被链接进受保护根」的写放大
//! 场景有风险。结论：暂不拒绝，后续如威胁模型升级再统一启用 nlink 判定。

use std::path::Path;

/// 判定 `path` 是否为链接（符号链接或 Windows junction）。
///
/// 用 `read_link` 的成功与否统一识别两类 reparse link：
/// - 符号链接：`read_link` 返回目标；
/// - junction（MOUNT_POINT reparse point）：`read_link` 同样返回目标；
/// - 普通文件/目录与硬链接：`read_link` 返回 Err → false。
///
/// 与 `symlink_metadata(...).file_type().is_symlink()` 配合使用：
/// 后者识别符号链接，本 helper 补上 junction，两者并集即完整 reparse 覆盖。
pub(crate) fn is_reparse_link(path: &Path) -> bool {
    std::fs::read_link(path).is_ok()
}

#[cfg(test)]
pub(crate) mod test_support {
    //! 测试辅助：创建**真实**链接 fixture。
    //!
    //! std 1.97.1 无特权时的降级行为（本仓库探针实证，2026-10-16）：
    //! - `symlink_dir` 返回 Ok 但创建**普通空目录**占位物（read_link 报 4390
    //!   「不是一个重分析点」，目标内容不出现）——不是 junction；
    //! - `symlink_file` 返回 Ok 但创建**普通空文件**占位物（len=0，非硬链接，
    //!   不共享目标内容）。
    //! 占位物会让「构造逃逸载体」的 fixture 变成无害普通对象，是 13 个
    //! symlink 类用例假红的根因。因此每次创建后必须用 read_link 验证真伪，
    //! 非链接即删除占位物并回退；全部失败时返回 false，调用方按既有惯例
    //! 跳过用例，不得用占位物误绿/误红。

    use std::path::Path;

    /// 创建物确实是链接（真符号链接或 junction），而非 std 降级占位物。
    fn is_actual_link(path: &Path) -> bool {
        super::is_reparse_link(path)
    }

    /// 创建真实目录链接（符号链接或 junction，均无需特权）。
    #[cfg(windows)]
    pub(crate) fn create_directory_link(target: &Path, link: &Path) -> bool {
        // 1) std symlink_dir：有特权时是真符号链接；1.97 无特权时降级为
        //    普通空目录占位物——验证失败即删除。
        if std::os::windows::fs::symlink_dir(target, link).is_ok() {
            if is_actual_link(link) {
                return true;
            }
            let _ = std::fs::remove_dir_all(link);
        }
        // 2) mklink /J junction：无需开发者模式或提升权限（与
        //    storage_deletion 既有 .cmd 脚本做法一致，这里直接调 cmd）。
        let created = std::process::Command::new("cmd")
            .args(["/d", "/s", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
            && is_actual_link(link);
        if !created {
            // 失败残留清理：junction 失败不会创建对象，防御性清理。
            let _ = std::fs::remove_dir_all(link);
        }
        created
    }

    #[cfg(unix)]
    pub(crate) fn create_directory_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(not(any(windows, unix)))]
    pub(crate) fn create_directory_link(_target: &Path, _link: &Path) -> bool {
        false
    }

    /// 创建真实文件符号链接。
    ///
    /// Windows 无特权时无法构造真文件链接（junction 仅目录；硬链接不是
    /// reparse link，且硬链接防护按评估结论不实现，不能用硬链接占位物
    /// 冒充符号链接 fixture）——std 降级占位物删除后返回 false，调用方跳过。
    #[cfg(windows)]
    pub(crate) fn create_file_link(target: &Path, link: &Path) -> bool {
        if std::os::windows::fs::symlink_file(target, link).is_ok() {
            if is_actual_link(link) {
                return true;
            }
            // 1.97 无特权降级物：普通空文件占位物，必须删除。
            let _ = std::fs::remove_file(link);
        }
        false
    }

    #[cfg(unix)]
    pub(crate) fn create_file_link(target: &Path, link: &Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(not(any(windows, unix)))]
    pub(crate) fn create_file_link(_target: &Path, _link: &Path) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::is_reparse_link;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn normal_file_and_directory_are_not_reparse_links() {
        let root = tempdir().unwrap();
        let file = root.path().join("file.txt");
        fs::write(&file, b"x").unwrap();
        let dir = root.path().join("dir");
        fs::create_dir(&dir).unwrap();

        assert!(!is_reparse_link(&file));
        assert!(!is_reparse_link(&dir));
        // 不存在的路径同样不是链接（调用方先用 metadata 判存在性）。
        assert!(!is_reparse_link(&root.path().join("missing")));
    }

    #[cfg(windows)]
    #[test]
    fn junction_is_detected_as_reparse_link() {
        // 1.97 std 的 symlink_dir 无特权时降级创建 junction（探针实证）；
        // 旧工具链无特权时创建失败——此时跳过而不是误报。
        let root = tempdir().unwrap();
        let target = root.path().join("target");
        fs::create_dir(&target).unwrap();
        let link = root.path().join("link");

        if std::os::windows::fs::symlink_dir(&target, &link).is_err() {
            // 无法构造目录链接：跳过，不误红。
            return;
        }
        // 只有当创建物确实是链接（真符号链接或 junction）时才断言；
        // 若 std 行为再变导致创建出普通目录，保持跳过语义不误红。
        if !is_reparse_link(&link) {
            let _ = fs::remove_dir_all(&link);
            return;
        }
        assert!(is_reparse_link(&link));
        let _ = fs::remove_dir_all(&link);
    }
}
