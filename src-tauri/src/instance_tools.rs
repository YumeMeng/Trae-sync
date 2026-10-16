//! 实例状态/进程工具族（P13-5 自 lib.rs 纯移动）。
//!
//! 收编组合根剩余的实例工具：启动公共链路 launch_instance_common、
//! profile→账号解析 trae_instance_account_id、凭据包实调判定
//! credential_login_state 及批量查询命令 get_trae_instance_states。
//! 主库/环境/库域经 `crate::instance_tools::` 引用（同 crate 内 mod 互引）。
//! 命令体、错误码与 wire 形态零变化。

use std::path::{Path, PathBuf};

use serde::Serialize;
use traesync_infrastructure::trae_instance::{
    self as trae_instance_module, default_product_data_dir, discover_trae_executable_for,
    focus_instance_windows, instance_data_dir, list_trae_processes, matched_product_process_ids,
    seed_login_state, TraeProcessInfo,
};
use traesync_infrastructure::{
    get_user_info_full, trae_http_client, AccountRegistry, CheckinCredentialStore,
    CheckinHttpError, CheckinProfileBinding, TraeProduct, UserInfoFull,
};

use crate::{checkin_material_root, run_blocking, AppState, RuntimeMode};

/// P2-2 实例启动结果：launched=本次新启动；focused=已在运行、已把窗口带到前台；
/// seeded=本次新启动且播种了原生目录登录态（前端可提示首次免登录）。
/// login_state=启动时刻的实例登录态（launched 分支据此引导未登录账号登录一次）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct TraeInstanceLaunchDto {
    /// pub(crate)：切号（credential_handover）与库域（library_archive）读取
    /// outcome 生成回执文案；login_state 仅随序列化出线。
    pub(crate) outcome: &'static str,
    login_state: trae_instance_module::InstanceLoginState,
}

/// 单账号登录凭据健康度（P7-5 凭据包实调判定）：
/// login_state = 凭据包 token 实调 GetUserInfo 的结果（与切号 E2 首选
/// 路径同源——徽章说有效则切号必通）：logged_in=实调成功 / stale=服务端
/// 拒绝或身份不一致 / uninitialized=无凭据包；断网时降级本地存档证据。
/// archive_available = 登录存档三件套登录键是否存在（E1 降级移植路径
/// 可用性，前端收进徽章悬浮提示的次要信息）。
/// pub(crate)：tauri 命令宏展开的包装函数签名需要暴露返回类型。
#[derive(Clone, Serialize)]
pub(crate) struct TraeInstanceStateDto {
    profile_id: String,
    login_state: trae_instance_module::InstanceLoginState,
    archive_available: bool,
}

/// P7-5 单账号凭据包实调判定（纯决策逻辑，实调用 `live_probe` 注入，
/// 供单测离线覆盖三态 + 断网降级）：
/// - 凭据包缺失/损坏 → 未登录（uninitialized）；
/// - 实调成功且 user_id 与凭据包一致 → 登录有效（logged_in）；
/// - 实调成功但身份不一致 / 服务端拒绝（401、业务码、协议异常）→ 登录失效（stale）；
/// - 网络层失败（断网）→ 无法实调，降级存档本地证据（与切号 E2→E1 同哲学）。
///
/// `archive_state` 为调用方预读的存档登录态（instance_login_state），
/// 登录键存在（logged_in/stale）即视为 E1 降级路径可用。
fn credential_login_state(
    store: &CheckinCredentialStore,
    record: &traesync_infrastructure::account_registry::AccountRecord,
    archive_state: trae_instance_module::InstanceLoginState,
    live_probe: &dyn Fn(&str) -> Result<UserInfoFull, CheckinHttpError>,
) -> (trae_instance_module::InstanceLoginState, bool) {
    let archive_available = matches!(
        archive_state,
        trae_instance_module::InstanceLoginState::LoggedIn
            | trae_instance_module::InstanceLoginState::Stale
    );
    let binding = CheckinProfileBinding::from(record);
    let Ok(bundle) = store.load(&binding) else {
        // 无凭据包 = 未登录（存档提示仍上报，老账号可能可走 E1 切换）。
        return (
            trae_instance_module::InstanceLoginState::Uninitialized,
            archive_available,
        );
    };
    match live_probe(&bundle.access_token) {
        Ok(info) if info.user_id == bundle.account_id => (
            trae_instance_module::InstanceLoginState::LoggedIn,
            archive_available,
        ),
        // 身份不一致 = 凭据错位不可信，与实调被拒同判失效。
        Ok(_) => (
            trae_instance_module::InstanceLoginState::Stale,
            archive_available,
        ),
        Err(CheckinHttpError::Network) => (archive_state, archive_available),
        Err(_) => (
            trae_instance_module::InstanceLoginState::Stale,
            archive_available,
        ),
    }
}

/// 校验 profile_id 并从账号注册表取 account_id（启动实例需要它做登录态种子）。
pub(crate) fn trae_instance_account_id(
    material_root: &Path,
    profile_id: &str,
) -> Result<(String, String), String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    records
        .iter()
        .find(|record| record.profile_id == profile_id)
        // 返回 (account_id, 窗口标题)：标题优先本地备注名，缺省回退服务端昵称
        // （A2：多开实例窗口按账号名区分，2026-08-29 trae-mate 对标裁定）。
        .map(|record| {
            (
                record.account_id.clone(),
                record
                    .display_name
                    .clone()
                    .unwrap_or_else(|| record.screen_name.clone()),
            )
        })
        .ok_or_else(|| "trae_profile_invalid".to_string())
}

/// 实例启动公共链路（P5-0 抽出，主库与账号实例共用）：
/// 进程查询 → 已运行则聚焦返回 → exe 发现 →（可选登录态种子）→ 建目录
/// → 窗口标题写入 → spawn。
/// 产品与数据目录共同决定实例归属；无法确认任一 TRAE 候选时停止，避免重复启动。
pub(crate) fn launch_instance_common(
    product: TraeProduct,
    instance_dir: &Path,
    window_title: &str,
    seed_account_id: Option<&str>,
) -> Result<TraeInstanceLaunchDto, String> {
    let processes = list_trae_processes().map_err(|e| e.code().to_string())?;
    launch_instance_common_with_processes(
        product,
        instance_dir,
        window_title,
        seed_account_id,
        &processes,
    )
}

/// 使用同一条启动编排消费进程快照，便于离线验证未知身份时不触发启动副作用。
fn launch_instance_common_with_processes(
    product: TraeProduct,
    instance_dir: &Path,
    window_title: &str,
    seed_account_id: Option<&str>,
    processes: &[TraeProcessInfo],
) -> Result<TraeInstanceLaunchDto, String> {
    launch_instance_common_with_processes_and_focus(
        product,
        instance_dir,
        window_title,
        seed_account_id,
        processes,
        focus_instance_windows,
    )
}

/// 将聚焦动作作为输入，便于测试验证 matcher 选出的 PID；生产入口仍传入系统聚焦函数。
fn launch_instance_common_with_processes_and_focus<F>(
    product: TraeProduct,
    instance_dir: &Path,
    window_title: &str,
    seed_account_id: Option<&str>,
    processes: &[TraeProcessInfo],
    mut focus_windows: F,
) -> Result<TraeInstanceLaunchDto, String>
where
    F: FnMut(&[u32]) -> bool,
{
    // 已在运行：聚焦目标进程；任何无法归属的候选都阻止新启动。
    let default_data_dir = default_product_data_dir(product);
    let running_pids = matched_product_process_ids(
        processes,
        product,
        instance_dir,
        default_data_dir.as_deref(),
    )
    .map_err(|_| "process_state_unknown".to_string())?;
    if !running_pids.is_empty() {
        focus_windows(&running_pids);
        return Ok(TraeInstanceLaunchDto {
            outcome: "focused",
            login_state: trae_instance_module::instance_login_state(instance_dir),
        });
    }

    let exe = discover_trae_executable_for(product, processes).map_err(|e| e.code().to_string())?;
    // 登录态种子仅账号实例使用；主库不播种（Q2 新建空主库：首启进 TRAE
    // 登录页登录一次，此后登录态由切号流程的凭据互换维护，P5-1）。
    let seed = seed_account_id
        .map(|account_id| seed_login_state(instance_dir, account_id))
        .transpose()
        .map_err(|e: trae_instance_module::TraeInstanceError| e.code().to_string())?;
    std::fs::create_dir_all(instance_dir)
        .map_err(|_| "trae_instance_dir_unavailable".to_string())?;
    // A2 窗口标题：启动前合并写入 User/settings.json 的 window.title
    // （--title CLI 对 TRAE 无效，settings.json 是实测可靠机制）。标题是
    // 便利功能，写入失败不阻断启动（trae-mate 同款处置）。
    let _ = trae_instance_module::write_window_title(instance_dir, window_title);
    // 参数交给 std 的 Windows 引用规则：路径含空格时自动整体加引号，
    // Chromium 系（TRAE 为 Electron）两种形态均可解析。
    //
    // stdio 重定向：TRAE(Electron) 的 Chromium 内部日志（ERROR/WARNING 白字）不再
    // 经继承的 stderr 刷入 dev 终端，而是追加到实例目录下的运行日志，排查 TRAE
    // 崩溃时仍有现场可查；同时把 cwd 指向实例目录，TRAE 的 debug.log 不再落到
    // src-tauri。日志文件打开失败（极端磁盘故障）时退回继承 stdio，不阻塞启动。
    let runtime_log = instance_dir.join("trae-runtime.log");
    let stdout_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&runtime_log)
        .ok()
        .map(std::process::Stdio::from);
    let stderr_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&runtime_log)
        .ok()
        .map(std::process::Stdio::from);
    let mut command = std::process::Command::new(&exe);
    command
        .arg(format!("--user-data-dir={}", instance_dir.display()))
        .current_dir(instance_dir);
    if let Some(handle) = stdout_handle {
        command.stdout(handle);
    }
    if let Some(handle) = stderr_handle {
        command.stderr(handle);
    }
    // P15-8 增补（2026-10-15 两次真实事故，用户报告）：工具进程消亡时，由
    // 本函数启动的 TRAE 客户端被连带终止——两条独立链条均已实证：
    // ① Job 继承链：子进程默认继承父进程所在 Job，kill-on-close 的 Job 随
    //    工具退出终止全部成员；
    // ② 控制台链（dev/debug 构建）：debug 版 trae-sync 附着启动它的控制台
    //    （一键启动 bat 的 cmd 窗口），spawn 的客户端继承同一控制台；用户
    //    关闭/重启 bat 窗口时 CTRL_CLOSE 发给整个控制台进程组，工具与
    //    客户端一起被终止（16:44 与 19:2x 两次事故的主链条）。
    // 处置：CREATE_BREAKAWAY_FROM_JOB（脱离 Job，需 Job 声明 BREAKAWAY_OK）
    // + DETACHED_PROCESS（不继承/不附着控制台）+ CREATE_NEW_PROCESS_GROUP
    // （脱离 Ctrl 信号组）。三重脱离后，客户端的存活与工具进程/控制台
    // 完全解耦；Job 拒绝脱离或极端失败时回退无标志启动（与历史行为一致）。
    // spawn 返回的 Child 句柄随即丢弃（std 不 kill），工具侧不持任何子
    // 进程生命周期。
    #[cfg(windows)]
    let spawned = {
        use std::os::windows::process::CommandExt;
        /// 脱离继承的 Job（需 Job 允许 BREAKAWAY_OK）。
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        /// 不继承/不附着控制台（GUI 程序无感知，免疫控制台关闭信号）。
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        /// 新进程组：脱离调用方的 Ctrl+C / Ctrl+Break 信号组。
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const DETACH_FLAGS: u32 =
            CREATE_BREAKAWAY_FROM_JOB | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        match command.creation_flags(DETACH_FLAGS).spawn() {
            Ok(child) => Ok(child),
            Err(_) => {
                command.creation_flags(0);
                command.spawn()
            }
        }
    };
    #[cfg(not(windows))]
    let spawned = command.spawn();
    spawned.map_err(|_| "trae_launch_failed".to_string())?;
    // 启动成功即清进程查询缓存：后续状态查询读到含新实例的真实快照。
    trae_instance_module::invalidate_process_cache();
    Ok(TraeInstanceLaunchDto {
        outcome: match seed {
            Some(trae_instance_module::SeedOutcome::Seeded) => "seeded",
            _ => "launched",
        },
        // seed 之后判定：播种成功即已登录；NativeMissing/空壳目录即待登录引导。
        login_state: trae_instance_module::instance_login_state(instance_dir),
    })
}

#[cfg(test)]
mod launch_process_guard_tests {
    use super::*;

    /// P14-2：聚焦回调只能收到匹配所选 Work 数据目录的 PID。
    #[test]
    fn focus_receives_only_pids_matching_the_selected_work_directory() {
        let root = tempfile::tempdir().unwrap();
        let instance_dir = root.path().join("selected-instance");
        let other_instance_dir = root.path().join("other-instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        let processes = [
            TraeProcessInfo {
                pid: 51,
                exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
                command_line: Some(format!(
                    r#""E:\TRAE SOLO CN.exe" --user-data-dir="{}""#,
                    instance_dir.display()
                )),
            },
            TraeProcessInfo {
                pid: 52,
                exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
                command_line: Some(format!(
                    r#""E:\TRAE SOLO CN.exe" --user-data-dir="{}""#,
                    other_instance_dir.display()
                )),
            },
        ];
        let mut focused_pids = Vec::new();
        let focus = |pids: &[u32]| {
            focused_pids.extend_from_slice(pids);
            true
        };

        let result = launch_instance_common_with_processes_and_focus(
            TraeProduct::WorkCn,
            &instance_dir,
            "测试环境",
            None,
            &processes,
            focus,
        )
        .unwrap();

        assert_eq!(result.outcome, "focused");
        assert_eq!(focused_pids, vec![51]);
    }

    /// P14-2：其他产品声称占用 Work 目录时必须 fail-closed，不能聚焦。
    #[test]
    fn cross_product_process_claiming_work_directory_blocks_focus() {
        let root = tempfile::tempdir().unwrap();
        let instance_dir = root.path().join("selected-instance");
        std::fs::create_dir_all(&instance_dir).unwrap();
        let processes = [
            TraeProcessInfo {
                pid: 61,
                exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
                command_line: Some(format!(
                    r#""E:\TRAE SOLO CN.exe" --user-data-dir="{}""#,
                    instance_dir.display()
                )),
            },
            TraeProcessInfo {
                pid: 63,
                exe_path: Some(r"E:\Trae CN\Trae CN.exe".to_string()),
                command_line: Some(format!(
                    r#""E:\Trae CN\Trae CN.exe" --user-data-dir="{}""#,
                    instance_dir.display()
                )),
            },
        ];
        let mut focused_pids = Vec::new();
        let focus = |pids: &[u32]| {
            focused_pids.extend_from_slice(pids);
            true
        };

        let result = launch_instance_common_with_processes_and_focus(
            TraeProduct::WorkCn,
            &instance_dir,
            "测试环境",
            None,
            &processes,
            focus,
        );

        assert!(matches!(result, Err(error) if error == "process_state_unknown"));
        assert!(focused_pids.is_empty());
    }

    #[test]
    fn unknown_work_process_identity_stops_before_creating_instance_directory() {
        let root = tempfile::tempdir().unwrap();
        let instance_dir = root.path().join("new-instance");
        let processes = [TraeProcessInfo {
            pid: 42,
            exe_path: Some(r"E:\TRAE SOLO CN.exe".to_string()),
            command_line: None,
        }];

        let result = launch_instance_common_with_processes(
            TraeProduct::WorkCn,
            &instance_dir,
            "测试环境",
            Some("profile-a"),
            &processes,
        );

        assert!(matches!(result, Err(error) if error == "process_state_unknown"));
        assert!(!instance_dir.exists());
    }
}

/// 批量查询账号登录凭据健康度（P7-5 凭据包实调判定）：
/// 逐账号并发实调 GetUserInfo（不后台轮询，仅账号页打开/手动刷新触发），
/// 断网降级本地存档证据；E1 存档可用性作为悬浮提示次要信息随行返回。
#[tauri::command]
pub(crate) async fn get_trae_instance_states(
    profile_ids: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<TraeInstanceStateDto>, String> {
    if state.runtime_mode != RuntimeMode::RealReadPreview {
        return Ok(profile_ids
            .into_iter()
            .map(|profile_id| TraeInstanceStateDto {
                profile_id,
                login_state: trae_instance_module::InstanceLoginState::Uninitialized,
                archive_available: false,
            })
            .collect());
    }
    let material_root = checkin_material_root(&state)?;
    let storage_root = PathBuf::from(&state.storage_root);
    run_blocking("trae_states_join_failed", move || {
        get_trae_instance_states_inner(&material_root, &storage_root, &profile_ids)
    })
    .await
}

fn get_trae_instance_states_inner(
    material_root: &Path,
    storage_root: &Path,
    profile_ids: &[String],
) -> Result<Vec<TraeInstanceStateDto>, String> {
    let registry = AccountRegistry::new(material_root);
    let records = registry
        .load()
        .map_err(|_| "checkin_registry_invalid".to_string())?;
    let store = CheckinCredentialStore::new(material_root);
    // 逐账号并发实调（scoped threads，HTTP 直连互不阻塞；账号量级个位数）。
    // 线程 panic 按「未登录」占位收尾，不让单账号异常拖垮整页徽章。
    let results = std::thread::scope(|scope| {
        let handles: Vec<(
            String,
            std::thread::ScopedJoinHandle<'_, TraeInstanceStateDto>,
        )> = profile_ids
            .iter()
            .map(|profile_id| {
                // 共享引用先行：move 闭包按引用捕获 records/store/storage_root，
                // 避免 FnMut 迭代时整体移出（E0507）。
                let records = &records;
                let store = &store;
                let storage_root = &*storage_root;
                let handle = scope.spawn(move || {
                    // DTO 需要所有权：&String 克隆为 String。
                    let profile_id = profile_id.clone();
                    let Some(record) = records
                        .iter()
                        .find(|record| record.profile_id == profile_id)
                    else {
                        return TraeInstanceStateDto {
                            profile_id,
                            login_state: trae_instance_module::InstanceLoginState::Uninitialized,
                            archive_available: false,
                        };
                    };
                    // 存档证据预读（断网降级判定 + E1 可用性）。
                    let archive_state = instance_data_dir(storage_root, &profile_id)
                        .map(|instance_dir| {
                            trae_instance_module::instance_login_state(&instance_dir)
                        })
                        .unwrap_or(trae_instance_module::InstanceLoginState::Uninitialized);
                    let live_probe = |token: &str| get_user_info_full(&trae_http_client(), token);
                    let (login_state, archive_available) =
                        credential_login_state(&store, record, archive_state, &live_probe);
                    TraeInstanceStateDto {
                        profile_id,
                        login_state,
                        archive_available,
                    }
                });
                // 元组侧单独克隆：闭包内的克隆已随 DTO 移走。
                (profile_id.clone(), handle)
            })
            .collect();
        handles
            .into_iter()
            .map(|(profile_id, handle)| {
                handle.join().unwrap_or(TraeInstanceStateDto {
                    profile_id,
                    login_state: trae_instance_module::InstanceLoginState::Uninitialized,
                    archive_available: false,
                })
            })
            .collect()
    });
    Ok(results)
}

/// P7-5 凭据包实调判定（credential_login_state）离线单测：
/// live_probe 注入，覆盖三态 + 断网降级 + 无凭据包 + 存档可用性随行。
/// P13-5 随域迁至 instance_tools 模块（纯移动）。
#[cfg(test)]
mod credential_login_state_tests {
    use super::*;

    /// 构造注册表档案 + 已保存凭据包的测试现场（真实 P-256 密钥，DPAPI 落盘）。
    fn fixture(
        root: &Path,
    ) -> (
        traesync_infrastructure::AccountRecord,
        CheckinCredentialStore,
    ) {
        let (private_pem, public_pem) = traesync_infrastructure::generate_device_keypair().unwrap();
        let record = traesync_infrastructure::AccountRecord {
            profile_id: "profile-health".to_string(),
            account_id: "700001".to_string(),
            screen_name: "测试账号".to_string(),
            avatar_url: String::new(),
            device_id: "1234567890123456".to_string(),
            device_public_key: public_pem.clone(),
            display_name: None,
            masked_mobile: String::new(),
            created_at_unix_seconds: 0,
            last_verified_at_unix_seconds: 0,
            device_created_at_unix_seconds: 0,
            auto_checkin_enabled: true,
            archived: false,
        };
        let store = CheckinCredentialStore::new(root);
        store
            .save(&traesync_infrastructure::CheckinCredentialBundle {
                profile_id: record.profile_id.clone(),
                account_id: record.account_id.clone(),
                device_id: record.device_id.clone(),
                machine_id: "machine-health".to_string(),
                device_public_key: public_pem,
                device_private_key: private_pem,
                access_token: "token-health".to_string(),
                refresh_token: "refresh-health".to_string(),
                client_id: "client-health".to_string(),
                access_token_expires_at_unix_seconds: u64::MAX,
                refresh_token_expires_at_unix_seconds: u64::MAX,
                mobile_full: None,
            })
            .unwrap();
        (record, store)
    }

    fn user_info(user_id: &str) -> UserInfoFull {
        UserInfoFull {
            user_id: user_id.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn probe_success_with_matching_identity_maps_to_logged_in() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        let (state, archive) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Ok(user_info("700001")),
        );
        // 徽章说有效则切号必通：实调成功且身份一致 = 登录有效。
        assert_eq!(state, trae_instance_module::InstanceLoginState::LoggedIn);
        assert!(archive);
    }

    #[test]
    fn probe_success_with_mismatched_identity_maps_to_stale() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        // 身份不一致 = 凭据错位不可信，与实调被拒同判失效。
        let (state, _) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Ok(user_info("999999")),
        );
        assert_eq!(state, trae_instance_module::InstanceLoginState::Stale);
    }

    #[test]
    fn server_rejection_maps_to_stale() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        let (state, _) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Err(CheckinHttpError::Business(20324)),
        );
        assert_eq!(state, trae_instance_module::InstanceLoginState::Stale);
    }

    #[test]
    fn network_failure_falls_back_to_archive_evidence() {
        let root = tempfile::tempdir().unwrap();
        let (record, store) = fixture(root.path());
        // 断网：无法实调，降级存档本地证据（与切号 E2→E1 同哲学）。
        let (state, archive) = credential_login_state(
            &store,
            &record,
            trae_instance_module::InstanceLoginState::Stale,
            &|_| Err(CheckinHttpError::Network),
        );
        assert_eq!(state, trae_instance_module::InstanceLoginState::Stale);
        assert!(archive);
    }

    #[test]
    fn missing_bundle_maps_to_uninitialized_but_archive_flag_survives() {
        let root = tempfile::tempdir().unwrap();
        let (record, _) = fixture(root.path());
        // 空仓库（未保存凭据包）：未登录，但存档可用性提示仍随行上报。
        let empty_store = CheckinCredentialStore::new(root.path().join("empty"));
        let (state, archive) = credential_login_state(
            &empty_store,
            &record,
            trae_instance_module::InstanceLoginState::LoggedIn,
            &|_| Ok(user_info("700001")),
        );
        assert_eq!(
            state,
            trae_instance_module::InstanceLoginState::Uninitialized
        );
        assert!(archive);
    }
}
