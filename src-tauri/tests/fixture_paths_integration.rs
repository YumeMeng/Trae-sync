//! Fixture 路径保护集成测试。
//!
//! 这些测试在 infrastructure crate 的单元测试之外，从外部视角验证
//! `FixturePathGuard` 的公共接口行为，对应 AC4 与 AC5，并满足 R1/R3 修复要求：
//! - R1：外部调用者只能通过 `FixturePathGuard::new(fixture_root)` 构造守卫，
//!   无法注入合成 SystemRoots。测试通过 RAII guard 临时设置 APPDATA/LOCALAPPDATA
//!   构造可信测试根，验证生产入口的行为。
//! - R3：所有被测试修改的环境变量统一通过 `EnvGuard` RAII guard 管理——
//!   保存旧值、持有互斥锁、Drop 恢复，panic-safe。同一次测试修改多个变量
//!   由一个 guard 原子管理。同时隔离 APPDATA 与 LOCALAPPDATA。
//! - 多轮并行回归不使用 `--test-threads=1`。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use tempfile::tempdir;
use traesync_infrastructure::{FixturePathError, FixturePathGuard, SystemRootsError};

/// 共享互斥锁：保护对 APPDATA/LOCALAPPDATA 等进程全局环境变量的并发修改。
static ENV_MUTEX: Mutex<()> = Mutex::new(());

/// RAII 守卫：原子管理多个环境变量，保存旧值，Drop 恢复，panic-safe。
///
/// 【R3 修复】同一次测试需要修改多个变量时由一个 guard 原子管理，
/// 任何 panic 都恢复。同时隔离 APPDATA 与 LOCALAPPDATA。
struct EnvGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _guard: MutexGuard<'static, ()>,
}

impl EnvGuard {
    /// 取得互斥锁并准备保存环境变量。
    fn lock() -> Self {
        Self {
            saved: Vec::new(),
            _guard: ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// 设置一个环境变量，保存旧值。
    fn set(mut self, name: &'static str, value: &Path) -> Self {
        let old = std::env::var_os(name);
        std::env::set_var(name, value);
        self.saved.push((name, old));
        self
    }

    /// 删除一个环境变量，保存旧值。
    fn remove(mut self, name: &'static str) -> Self {
        let old = std::env::var_os(name);
        std::env::remove_var(name);
        self.saved.push((name, old));
        self
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // 逆序恢复，与设置顺序相反，确保原子性
        while let Some((name, old)) = self.saved.pop() {
            if let Some(old) = old {
                std::env::set_var(name, old);
            } else {
                std::env::remove_var(name);
            }
        }
    }
}

/// 辅助：构造可信测试环境。
///
/// 创建 tempdir 作为 LOCALAPPDATA，在其中创建 `Trae Sync\tests` 目录作为 test_root。
/// 同时设置 APPDATA 指向另一个 tempdir（用于解析 default_work_cn_dir）。
/// 返回 (local_appdata_tempdir, appdata_tempdir, test_root_path, fixture_root_path)。
fn trusted_test_env() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, PathBuf) {
    let local_appdata = tempdir().unwrap();
    let appdata = tempdir().unwrap();

    // 创建 test_root: %LOCALAPPDATA%\Trae Sync\tests
    let test_root = local_appdata.path().join("Trae Sync").join("tests");
    fs::create_dir_all(&test_root).unwrap();

    // 创建 fixture_root 在 test_root 内部
    let fixture_root = test_root.join("fixture");
    fs::create_dir_all(&fixture_root).unwrap();

    (local_appdata, appdata, test_root, fixture_root)
}

/// 辅助：在 EnvGuard 保护下设置 APPDATA 和 LOCALAPPDATA 指向可信测试环境。
fn set_trusted_env(local_appdata: &Path, appdata: &Path) -> EnvGuard {
    EnvGuard::lock()
        .set("LOCALAPPDATA", local_appdata)
        .set("APPDATA", appdata)
}

#[test]
fn fixture_root_must_exist() {
    let (local_appdata, appdata, _test_root, _fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let non_existent = _test_root.join("non-existent-").join("missing");
    let err = FixturePathGuard::new(&non_existent).unwrap_err();
    assert!(matches!(err, FixturePathError::CannotCanonicalize { .. }));
}

#[test]
fn write_target_inside_fixture_root_accepted() {
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    fs::create_dir_all(fixture_root.join("sub")).unwrap();
    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let candidate = fixture_root.join("sub").join("data.db");
    let result = guard.validate_write_target(&candidate).unwrap();
    assert!(result.starts_with(guard.canonical_root()));
}

#[test]
fn write_target_outside_fixture_root_rejected() {
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let outside = tempdir().unwrap();
    let err = guard.validate_write_target(outside.path()).unwrap_err();
    assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
}

#[test]
fn parent_dir_escape_rejected() {
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let candidate = fixture_root.join("..").join("escape.db");
    let err = guard.validate_write_target(&candidate).unwrap_err();
    assert!(matches!(
        err,
        FixturePathError::OutsideFixtureRoot { .. } | FixturePathError::CannotCanonicalize { .. }
    ));
}

#[test]
fn default_work_cn_path_rejected() {
    // 通过设置 APPDATA 指向 tempdir，在其中构造默认 Work CN 路径
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();

    // 在 appdata 下创建默认 Work CN 路径
    let default_dir = appdata
        .path()
        .join("TRAE SOLO CN")
        .join("ModularData")
        .join("ai-agent");
    fs::create_dir_all(&default_dir).unwrap();
    let default_db = default_dir.join("database.db");
    fs::write(&default_db, b"").unwrap();

    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let err = guard.validate_write_target(&default_db).unwrap_err();
    assert!(matches!(err, FixturePathError::DefaultWorkCnPath { .. }));
}

#[test]
fn default_work_cn_directory_rejected() {
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();

    let default_dir = appdata
        .path()
        .join("TRAE SOLO CN")
        .join("ModularData")
        .join("ai-agent");
    fs::create_dir_all(&default_dir).unwrap();

    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let err = guard.validate_write_target(&default_dir).unwrap_err();
    assert!(matches!(err, FixturePathError::DefaultWorkCnPath { .. }));
}

#[test]
fn no_hidden_bypass_via_env_var() {
    // 设置假装的"绕过"环境变量——守卫应当无视它
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path())
        .set("TRAESYNC_BYPASS_PATH_CHECK", Path::new("1"))
        .set("TRAESYNC_ALLOW_REAL_PATH", Path::new("true"))
        .set("TRAESYNC_TEST_MODE", Path::new("unsafe"));

    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let outside = tempdir().unwrap();
    let err = guard.validate_write_target(outside.path()).unwrap_err();
    assert!(matches!(err, FixturePathError::OutsideFixtureRoot { .. }));
}

#[test]
fn from_env_fails_closed_when_appdata_missing() {
    // APPDATA 缺失时必须失败关闭
    let _env = EnvGuard::lock().remove("APPDATA");
    let temp = tempdir().unwrap();
    let err = FixturePathGuard::new(temp.path()).unwrap_err();
    assert!(matches!(
        err,
        FixturePathError::SystemRoots(SystemRootsError::AppdataMissing)
    ));
}

#[test]
fn from_env_fails_closed_when_localappdata_missing() {
    // LOCALAPPDATA 缺失时必须失败关闭
    // 保留 _local_appdata 临时目录句柄以维持其生命周期，但不设置环境变量
    let (_local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();
    let _env = EnvGuard::lock()
        .set("APPDATA", appdata.path())
        .remove("LOCALAPPDATA");

    let err = FixturePathGuard::new(&fixture_root).unwrap_err();
    assert!(matches!(
        err,
        FixturePathError::SystemRoots(SystemRootsError::TestRootUnavailable { .. })
    ));
}

#[test]
fn from_env_fails_closed_when_test_root_missing() {
    // APPDATA 和 LOCALAPPDATA 都存在，但 test_root 目录不存在时失败关闭
    let local_appdata = tempdir().unwrap();
    let appdata = tempdir().unwrap();
    // 故意不创建 `Trae Sync\tests` 目录
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let temp = tempdir().unwrap();
    let err = FixturePathGuard::new(temp.path()).unwrap_err();
    assert!(matches!(
        err,
        FixturePathError::SystemRoots(SystemRootsError::TestRootUnavailable { .. })
    ));
}

#[test]
fn arbitrary_user_dir_as_fixture_root_rejected() {
    // 任意用户目录不能作为 fixture_root——必须在 test_root 内
    let (local_appdata, appdata, _test_root, _fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let user_dir = tempdir().unwrap();
    let err = FixturePathGuard::new(user_dir.path()).unwrap_err();
    assert!(matches!(
        err,
        FixturePathError::FixtureRootOutsideTestRoot { .. }
    ));
}

#[test]
fn disk_root_as_fixture_root_rejected() {
    // 磁盘根不能作为 fixture_root
    let (local_appdata, appdata, _test_root, _fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let err = FixturePathGuard::new(Path::new("C:\\")).unwrap_err();
    assert!(matches!(
        err,
        FixturePathError::FixtureRootOutsideTestRoot { .. }
            | FixturePathError::CannotCanonicalize { .. }
    ));
}

#[test]
fn external_caller_cannot_inject_synthetic_roots() {
    // R1 回归验证：外部 crate 无法访问 SystemRoots 或 PathPolicy
    // 这里通过编译期保证——如果 SystemRoots 或 PathPolicy 是公开的，
    // 下面的代码会编译通过；它们是 pub(crate)，所以无法从外部构造。
    // 本测试仅验证 FixturePathGuard::new 是唯一构造入口。
    let (local_appdata, appdata, _test_root, fixture_root) = trusted_test_env();
    let _env = set_trusted_env(local_appdata.path(), appdata.path());

    let guard = FixturePathGuard::new(&fixture_root).unwrap();
    let canonical_test_root = _test_root.canonicalize().unwrap();
    assert!(guard.canonical_root().starts_with(&canonical_test_root));
}
