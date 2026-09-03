// ============================================================================
// Playwright mock 命令边界（P5-3 历史页主库视图）
// ============================================================================
//
// 关键约束：
// - 使用确定性 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据
// - 通过 page.addInitScript 在应用加载前安装 window.__TAURI_INTERNALS__.invoke
// - 所有返回数据为合成 fixture，不含真实账号/会话/认证正文
//
// Tauri 2 中 @tauri-apps/api/core 的 invoke 最终调用
// window.__TAURI_INTERNALS__.invoke(cmd, args, options)，因此 mock 该入口即可。

import type { Page } from "@playwright/test";

// P5-3 主库历史读取状态（get_master_history.status 同构）。
export type MasterHistoryMode =
  | "ready"
  | "no_master_data"
  | "no_current_account"
  | "read_failed";

// 场景配置：每个场景决定各命令的返回
export interface MockScenario {
  // RealReadPreview：生产只读 workspace，使用默认位置与命令。
  production?: boolean;
  // P5-3：主库历史读取状态（默认 ready，两栏数据可见）。
  masterHistory?: MasterHistoryMode;
}

/**
 * 在页面加载前安装 mock invoke。
 * 测试通过 setScenario 切换场景，避免每个测试重新加载页面。
 */
export async function installMockBridge(
  page: Page,
  scenario: MockScenario = {},
) {
  // 把场景容器序列化注入页面，作为初始 bridge 状态。
  await page.addInitScript(({ scenario: initialScenario }) => {
    // 场景容器——测试运行时可通过 window.__setScenario 更新
    (window as any).__activeScenario = initialScenario;
    (window as any).__setScenario = (s: unknown) => {
      (window as any).__activeScenario = s;
    };

    // 预定义合成数据
    const PRODUCTION_LOCATION = "C:\\TRAE\\ModularData";
    const PRODUCTION_WS = {
      platform: {
        platform_id: "work_cn",
        display_name: "TRAE Work CN",
        adapter_implemented: true,
      },
      data_location: {
        selected: true,
        display_name: PRODUCTION_LOCATION,
        unavailable_reason: null,
      },
      current_account: {
        detected: false,
        user_fingerprint: null,
        unavailable_reason: "not_detected",
      },
      history: { account_count: 0, project_count: 0, session_count: 0 },
      capabilities: {
        scan_enabled: true,
        sync_enabled: false,
        backup_enabled: false,
        restore_enabled: false,
      },
      honest_status: "RealReadPreview fixture mock",
    };

    const WS = {
      platform: {
        platform_id: "work_cn",
        display_name: "TRAE Work CN",
        adapter_implemented: false,
      },
      data_location: {
        selected: false,
        display_name: null,
        unavailable_reason: "not_selected",
      },
      current_account: {
        detected: false,
        user_fingerprint: null,
        unavailable_reason: "not_detected",
      },
      history: { account_count: 0, project_count: 0, session_count: 0 },
      capabilities: {
        scan_enabled: true,
        sync_enabled: false,
        backup_enabled: false,
        restore_enabled: false,
      },
      honest_status: "真实能力尚未启用",
    };

    // 账号中心只提供非敏感档案，供设置页 E2E 验证实际挂载和重新检测入口。
    const MANAGED_ACCOUNT_STATE: any = {
      saved_accounts: [
        {
          profile_id: "profile-current",
          display_name: "工作账号 A",
          region: "cn",
          data_location_id: "loc-fixture",
          last_verified_at: "2025-07-01T00:00:00.000Z",
          verification_state: "verified",
        },
        {
          profile_id: "profile-target",
          display_name: "工作账号 B",
          region: "cn",
          data_location_id: "loc-fixture",
          last_verified_at: "2025-07-01T00:00:00.000Z",
          verification_state: "verified",
        },
      ],
      current_account: {
        profile_id: "profile-current",
        display_name: "工作账号 A",
        region: "cn",
        data_location_id: "loc-fixture",
        verification_state: "verified",
        observed_at: "2025-07-01T00:00:00.000Z",
        reason: null,
      },
      recent_verification: {
        verification_state: "verified",
        checked_at: "2025-07-01T00:00:00.000Z",
        reason: null,
      },
      switch_state: null,
      handoff_intent: null,
      history_is_separate: true,
    };

    const KEY_STATUS = {
      source_key_configured: true,
      source_key_version: "work-cn-baseline-v1",
      source_key_pending_version: null,
      source_key_activation_pending: false,
      catalog_key_configured: true,
      catalog_key_generation: 1,
      probe_state: "not_probed",
    };

    // init script 内共享时间基准（P5-3 fixture 相对时间都用它）。
    const NOW_LOCAL = Math.floor(Date.now() / 1000);
    const DAY = 86400;

    // 签到页/账号页总览 fixture（结构与 CheckinOverviewEntryDto 同构）。
    const CHECKIN_OVERVIEW = [
      {
        profile_id: "profile-current",
        account_id: "user-B",
        screen_name: "工作账号 B",
        created_at: "2026-08-01T00:00:00Z",
        last_verified_at: "2026-08-26T00:00:00Z",
        credits: 200,
        credits_cached_at: "2026-08-26T01:00:00Z",
        usage_remaining_credits: 1600,
        usage_cached_at: "2026-08-26T01:00:00Z",
        checked_in: true,
        access_token_expires_at_unix_seconds: NOW_LOCAL + 13 * 86400,
        refresh_token_expires_at_unix_seconds: NOW_LOCAL + 179 * 86400,
        device_tail: "9012",
        device_id: "3569646294624771",
        display_name: null,
        masked_mobile: "156******19",
        auto_checkin_enabled: true,
      },
      {
        profile_id: "profile-target",
        account_id: "user-A",
        screen_name: "工作账号 A",
        created_at: "2026-08-02T00:00:00Z",
        last_verified_at: "2026-08-26T00:00:00Z",
        credits: 200,
        credits_cached_at: "2026-08-26T01:00:00Z",
        usage_remaining_credits: 1400,
        usage_cached_at: "2026-08-26T01:00:00Z",
        checked_in: false,
        access_token_expires_at_unix_seconds: NOW_LOCAL + 8 * 86400,
        refresh_token_expires_at_unix_seconds: NOW_LOCAL + 170 * 86400,
        device_tail: "7701",
        device_id: "2229135200000002",
        display_name: null,
        masked_mobile: "158******27",
        auto_checkin_enabled: true,
      },
    ];

    // ===== P5-3 主库历史 fixture：两项目 + 三会话（含一条三跳接力链）=====

    // 环境页 mock 用的账号名录（profile ↔ user_id ↔ 显示名）。
    const ACCOUNTS = [
      { profile_id: "profile-current", account_id: "user-B", screen_name: "工作账号 B" },
      { profile_id: "profile-target", account_id: "user-A", screen_name: "工作账号 A" },
    ];

    // 会话 s1 的三跳接力链：A→B（s1-a）、B→A（s1-b）、A→B（s1）。
    // 结束腿主人 = user-B = 主库当前账号（与可见性过滤口径一致）。
    const RELAY_LEDGER = [
      {
        session_id: "s1-a",
        from_session_id: null,
        project_id: "p1",
        from_user_id: "user-A",
        from_account_name: "工作账号 A",
        to_user_id: "user-B",
        to_account_name: "工作账号 B",
        message_count_at_switch: 4,
        switched_at_unix_seconds: NOW_LOCAL - 6 * DAY,
      },
      {
        session_id: "s1-b",
        from_session_id: "s1-a",
        project_id: "p1",
        from_user_id: "user-B",
        from_account_name: "工作账号 B",
        to_user_id: "user-A",
        to_account_name: "工作账号 A",
        message_count_at_switch: 7,
        switched_at_unix_seconds: NOW_LOCAL - 3 * DAY,
      },
      {
        session_id: "s1",
        from_session_id: "s1-b",
        project_id: "p1",
        from_user_id: "user-A",
        from_account_name: "工作账号 A",
        to_user_id: "user-B",
        to_account_name: "工作账号 B",
        message_count_at_switch: 9,
        switched_at_unix_seconds: NOW_LOCAL - DAY,
      },
    ];

    // 显式注解：hidden_status 归档/恢复会在 mock 命令中改写为 null，不能收窄为 string。
    const MASTER_HISTORY_READY: {
      status: string;
      current_user_id: string;
      projects: Array<{ project_id: string; name: string; absolute_path: string | null }>;
      sessions: Array<{
        session_id: string;
        project_id: string;
        title: string;
        message_count: number;
        updated_at_unix_seconds: number;
        deleted: boolean;
        hidden_status: string | null;
        work_mode: string | null;
      }>;
      fingerprint: object;
    } = {
      status: "ready",
      current_user_id: "user-B",
      projects: [
        { project_id: "p1", name: "项目阿尔法", absolute_path: null },
        { project_id: "p2", name: "项目贝塔", absolute_path: null },
      ],
      sessions: [
        {
          session_id: "s1",
          project_id: "p1",
          title: "会话一 关于构建稳定的历史库",
          message_count: 12,
          updated_at_unix_seconds: NOW_LOCAL - 3600,
          deleted: false,
          hidden_status: null,
          work_mode: "code",
        },
        {
          session_id: "s2",
          project_id: "p1",
          title: "会话二 软删除与版本图",
          message_count: 5,
          updated_at_unix_seconds: NOW_LOCAL - 2 * DAY,
          deleted: false,
          hidden_status: null,
          work_mode: "code",
        },
        {
          session_id: "s3",
          project_id: "p2",
          title: "旧归档会话",
          message_count: 3,
          updated_at_unix_seconds: NOW_LOCAL - 40 * DAY,
          deleted: false,
          // P5-8a：voice_discussion 借用为归档 → 不入主列表，进归档视图。
          hidden_status: "voice_discussion",
          work_mode: "work",
        },
        {
          // 已删除会话：右栏必须过滤（deleted=false 的三会话可见）。
          session_id: "s4",
          project_id: "p1",
          title: "已删除会话",
          message_count: 2,
          updated_at_unix_seconds: NOW_LOCAL - 3600,
          deleted: true,
          hidden_status: null,
          work_mode: null,
        },
      ],
      fingerprint: {
        db: { mtime_secs: NOW_LOCAL - 60, mtime_nanos: 0, size: 1024 },
        wal: { mtime_secs: NOW_LOCAL - 60, mtime_nanos: 0, size: 512 },
        shm: null,
      },
    };

    // 主库消息预览 fixture（s1 文本 + 任务轨迹两种形态；其余会话单条文本）。
    function makeMasterMessages(sessionId: string) {
      if (sessionId === "s1") {
        return {
          session_id: sessionId,
          status: "ready",
          messages: [
            {
              message_id: "m1",
              role: "user",
              message_type: "general",
              created_at_unix_seconds: NOW_LOCAL - 2 * DAY,
              content: { kind: "text", text: "如何跨账号接力这条会话？", step_count: 0, thoughts: [] },
            },
            {
              message_id: "m2",
              role: "assistant",
              message_type: "general",
              created_at_unix_seconds: NOW_LOCAL - 2 * DAY + 60,
              content: { kind: "text", text: "通过主库交接把记录转移给接收账号。", step_count: 0, thoughts: [] },
            },
            {
              message_id: "m3",
              role: "assistant",
              message_type: "task",
              created_at_unix_seconds: NOW_LOCAL - DAY,
              content: {
                kind: "task_trace",
                text: "",
                step_count: 2,
                thoughts: ["检索主库会话索引", "写入接力台账"],
              },
            },
          ],
        };
      }
      return {
        session_id: sessionId,
        status: "ready",
        messages: [
          {
            message_id: `m-${sessionId}`,
            role: "user",
            message_type: "general",
            created_at_unix_seconds: NOW_LOCAL - DAY,
            content: { kind: "text", text: `会话 ${sessionId} 的合成消息。`, step_count: 0, thoughts: [] },
          },
        ],
      };
    }

    // P5-2：主库环境档案 mock（get_environment_state / launch_master_library /
    // switch_master_account）。切号成功后当前账号原地转移，供「使用中」标记断言。
    let envCurrentProfileId: string | null = "profile-current";
    // P5-4：主库备份链 mock（初始 2 份；手动备份追加）。
    const NOW = Math.floor(Date.now() / 1000);
    let masterBackups = [
      { stamp_unix_seconds: NOW - 86400, total_bytes: 2 * 1024 * 1024, has_wal: true },
      { stamp_unix_seconds: NOW - 3 * 86400, total_bytes: 512 * 1024, has_wal: false },
    ];

    // P5-8b：插件 tab 可变状态（装/卸/吸收直接改，刷新后列表与对账条联动）。
    // 必须声明在 mockInvoke 外：函数体内的 let 每次调用都会重新初始化，
    // 状态无法跨调用保持（对账吸收断言会失败）。
    let pluginInstalled = [
      {
        record_id: "plug-rec-1",
        marketplace_plugin_id: "plug-uuid-1",
        name: "lark-suite",
        display_name: "飞书协作",
        version: "1.4.0",
        registry: "trae-remote-official",
        builtin: false,
      },
      {
        record_id: "plug-rec-2",
        marketplace_plugin_id: null,
        name: "builtin-browser",
        display_name: "浏览器控制",
        version: "0.0.0",
        registry: "builtin",
        builtin: true,
      },
      // 对账偏移：TRAE 内手动装（云端有、清单无）。
      {
        record_id: "plug-rec-3",
        marketplace_plugin_id: "plug-uuid-3",
        name: "seedance",
        display_name: "视频生成",
        version: "1.0.2",
        registry: "trae-remote-official",
        builtin: false,
      },
    ];
    let pluginManifest = [
      {
        marketplace_plugin_id: "plug-uuid-1",
        name: "lark-suite",
        display_name: "飞书协作",
        version: "1.4.0",
        registry: "trae-remote-official",
        installed_in_cloud: true,
      },
      // 对账偏移：TRAE 内手动卸（清单有、云端无）。
      {
        marketplace_plugin_id: "plug-uuid-4",
        name: "seedream",
        display_name: "图片生成",
        version: "0.9.1",
        registry: "trae-remote-official",
        installed_in_cloud: false,
      },
    ];
    const PLUGIN_MARKET = [
      {
        plugin_id: "plug-uuid-1",
        name: "lark-suite",
        display_name: "飞书协作",
        description: "合成市场条目：消息、文档与审批",
        registry: "trae-remote-official",
        categories: ["collaboration"],
        category_key: "collaboration",
        category_name: "协作工具",
      },
      {
        plugin_id: "plug-uuid-5",
        name: "code-graph",
        display_name: "代码图谱",
        description: "合成市场条目：仓库结构可视化",
        registry: "trae-remote-official",
        categories: ["devtools"],
        category_key: "devtools",
        category_name: "开发工具",
      },
    ];

    // mock invoke 实现：根据 activeScenario 返回合成数据
    async function mockInvoke(cmd: string, args?: any) {
      const scn = (window as any).__activeScenario || {};
      if (cmd === "get_workspace_state") {
        if (scn.production) return PRODUCTION_WS;
        return WS;
      }
      if (cmd === "get_managed_account_state") return MANAGED_ACCOUNT_STATE;
      if (cmd === "get_key_status") return KEY_STATUS;
      if (cmd === "get_checkin_capability") {
        return {
          enabled: !scn.production,
          transport: scn.production ? "disabled" : "fixture",
          real_http_enabled: false,
          message: scn.production
            ? "真实签到未启用；当前版本只开放主库历史与账号切换。"
            : "当前为 fixture transport，仅用于验收签到流程；不会访问远程服务。",
        };
      }
      if (cmd === "get_checkin_overview") return CHECKIN_OVERVIEW;
      if (cmd === "get_trae_instance_states") {
        return (args?.profileIds ?? []).map((profileId: string) => ({
          profile_id: profileId,
          running: false,
          login_state: "uninitialized",
        }));
      }
      if (cmd === "launch_trae_instance") {
        return { outcome: "launched", login_state: "logged_in" };
      }
      if (cmd === "close_trae_instance") {
        return { outcome: "closed" };
      }
      // P5-2：主库环境命令（环境页数据源 + 切号弹层回执）。
      if (cmd === "get_environment_state") {
        const current = ACCOUNTS.find((entry) => entry.profile_id === envCurrentProfileId);
        return {
          env_id: "master",
          current_profile_id: envCurrentProfileId,
          current_account_name: current?.screen_name ?? null,
          data_dir: "C:\\TraeSync\\data\\environments\\master",
          running: false,
          login_state: "logged_in",
          created_at_unix_seconds: 1750000000,
        };
      }
      if (cmd === "launch_master_library") {
        return { outcome: "launched", login_state: "logged_in" };
      }
      if (cmd === "preview_master_switch_plugins") {
        // 默认无差异（aborted）：弹层静默直过切号；确认分支由组件测试覆盖。
        return {
          source_count: 0,
          target_count: 0,
          install_names: [],
          remove_names: [],
          aborted: true,
        };
      }
      if (cmd === "switch_master_account") {
        const target = ACCOUNTS.find((entry) => entry.profile_id === args?.profileId);
        if (!target) throw new Error("trae_profile_invalid");
        const fromProfileId = envCurrentProfileId;
        envCurrentProfileId = target.profile_id;
        return {
          profile_id: target.profile_id,
          to_user_id: target.account_id,
          from_user_id: fromProfileId,
          transferred_projects: 2,
          removed_mirror_rows: 0,
          switched_sessions: 3,
          backup_path: "C:\\TraeSync\\data\\environments\\master\\.switch-bak-20260831",
          relay_ledger_written: true,
          plugin_sync: {
            source_count: 0,
            installed: 0,
            removed: 0,
            failed: 0,
            skipped: 0,
            aborted: false,
            declined: false,
          },
          relaunch_outcome: "launched",
        };
      }
      // P5-4 主库统计与备份链（环境页统计格 / 设置页备份分区数据源）。
      if (cmd === "get_master_library_stats") {
        return {
          status: "ready",
          current_user_id: ACCOUNTS.find((entry) => entry.profile_id === envCurrentProfileId)?.account_id ?? null,
          project_count: 4,
          session_count: 16,
          message_count: 128,
          participating_account_count: 3,
          last_active_unix_seconds: Math.floor(Date.now() / 1000) - 3600,
          size_bytes: 712 * 1024 * 1024,
        };
      }
      if (cmd === "get_master_backup_chain") {
        return { backups: [...masterBackups], backup_dir: "C:\\TraeSync\\data\\environments\\master\\ModularData\\ai-agent", keep_policy: 5 };
      }
      if (cmd === "create_master_backup") {
        // 手动备份语义：追加新条目（create_new 永不覆盖），徽章计数随之 +1。
        masterBackups.push({
          stamp_unix_seconds: Math.floor(Date.now() / 1000),
          total_bytes: 3 * 1024 * 1024,
          has_wal: false,
        });
        return "C:\\TraeSync\\data\\environments\\master\\ModularData\\ai-agent\\.switch-bak-manual";
      }
      // ===== P5-3 历史页主库视图 =====
      if (cmd === "get_master_history") {
        const mode = scn.masterHistory ?? "ready";
        // 指纹预检：previous 与当前指纹一致 → unchanged（前端维持现有列表）。
        if (
          mode === "ready" &&
          args?.previous &&
          JSON.stringify(args.previous) === JSON.stringify(MASTER_HISTORY_READY.fingerprint)
        ) {
          return {
            status: "unchanged",
            current_user_id: null,
            projects: [],
            sessions: [],
            fingerprint: MASTER_HISTORY_READY.fingerprint,
          };
        }
        if (mode === "ready") {
          // 返回浅拷贝（新引用）：归档/恢复/删除 mock 会改写 sessions，
          // 同一引用会被 React 状态比较 bail out，列表不刷新。
          return {
            ...MASTER_HISTORY_READY,
            sessions: MASTER_HISTORY_READY.sessions.map((session) => ({ ...session })),
          };
        }
        return {
          status: mode,
          current_user_id: null,
          projects: [],
          sessions: [],
          fingerprint: MASTER_HISTORY_READY.fingerprint,
        };
      }
      if (cmd === "get_relay_ledger") return RELAY_LEDGER;
      // ===== P5-8a 会话归档三命令（主列表归档 / 归档视图恢复与删除）=====
      // 可变副本：归档/恢复直接改 hidden_status，刷新后两栏联动（与真机行为同构）。
      if (cmd === "archive_master_sessions") {
        const ids = new Set(Array.isArray(args?.sessionIds) ? args.sessionIds : []);
        for (const session of MASTER_HISTORY_READY.sessions) {
          if (ids.has(session.session_id) && !session.deleted) {
            session.hidden_status = "voice_discussion";
          }
        }
        return { affected: ids.size };
      }
      if (cmd === "restore_master_sessions") {
        const ids = new Set(Array.isArray(args?.sessionIds) ? args.sessionIds : []);
        let affected = 0;
        for (const session of MASTER_HISTORY_READY.sessions) {
          if (ids.has(session.session_id) && session.hidden_status === "voice_discussion") {
            session.hidden_status = null;
            affected += 1;
          }
        }
        return { affected };
      }
      if (cmd === "delete_master_sessions") {
        const ids = new Set(Array.isArray(args?.sessionIds) ? args.sessionIds : []);
        let deletedMessages = 0;
        MASTER_HISTORY_READY.sessions = MASTER_HISTORY_READY.sessions.filter(
          (session: { session_id: string; message_count: number }) => {
            if (ids.has(session.session_id)) {
              deletedMessages += session.message_count;
              return false;
            }
            return true;
          },
        );
        return {
          deleted_sessions: ids.size,
          deleted_messages: deletedMessages,
          removed_projects: 0,
          backup_path: "C:\\TraeSync\\data\\environments\\master\\ModularData\\ai-agent\\.switch-bak-del",
        };
      }
      if (cmd === "get_master_session_messages") {
        return makeMasterMessages(String(args?.sessionId ?? ""));
      }
      // ===== P5-8b 插件 tab（ADR-0023 环境插件清单）=====
      // 状态与市场目录见 mockInvoke 外的 pluginInstalled/pluginManifest/PLUGIN_MARKET。
      if (cmd === "get_plugin_tab_state") {
        const manifestIds = new Set(pluginManifest.map((entry) => entry.marketplace_plugin_id));
        const cloudIds = new Set(
          pluginInstalled
            .map((item) => item.marketplace_plugin_id)
            .filter((id) => id !== null),
        );
        return {
          installed: pluginInstalled.map((item) => ({
            ...item,
            in_manifest:
              item.marketplace_plugin_id !== null &&
              manifestIds.has(item.marketplace_plugin_id),
          })),
          manifest: pluginManifest.map((entry) => ({
            ...entry,
            installed_in_cloud: cloudIds.has(entry.marketplace_plugin_id),
          })),
        };
      }
      if (cmd === "browse_plugin_market") return PLUGIN_MARKET;
      if (cmd === "install_plugin") {
        const pluginId = String(args?.pluginId ?? "");
        const market = PLUGIN_MARKET.find((entry) => entry.plugin_id === pluginId);
        if (!market) throw new Error("plugin_install_failed");
        pluginInstalled = [
          ...pluginInstalled,
          {
            record_id: `plug-rec-${pluginId}`,
            marketplace_plugin_id: pluginId,
            name: market.name,
            display_name: market.display_name,
            version: "1.0.0",
            registry: market.registry,
            builtin: false,
          },
        ];
        pluginManifest = [
          ...pluginManifest,
          {
            marketplace_plugin_id: pluginId,
            name: market.name,
            display_name: market.display_name,
            version: "1.0.0",
            registry: market.registry,
            installed_in_cloud: true,
          },
        ];
        return null;
      }
      if (cmd === "uninstall_plugin") {
        const recordId = String(args?.recordId ?? "");
        const item = pluginInstalled.find((entry) => entry.record_id === recordId);
        if (!item) throw new Error("plugin_not_found");
        if (item.builtin) throw new Error("plugin_builtin_uninstallable");
        pluginInstalled = pluginInstalled.filter((entry) => entry.record_id !== recordId);
        pluginManifest = pluginManifest.filter(
          (entry) => entry.marketplace_plugin_id !== item.marketplace_plugin_id,
        );
        return null;
      }
      if (cmd === "absorb_plugin_manifest") {
        // 吸收语义：以当前账号云端现状为准重写清单（手动装的入清单，手动卸的出清单）。
        pluginManifest = pluginInstalled
          .filter((item) => !item.builtin && item.marketplace_plugin_id !== null)
          .map((item) => ({
            marketplace_plugin_id: item.marketplace_plugin_id as string,
            name: item.name,
            display_name: item.display_name,
            version: item.version,
            registry: item.registry,
            installed_in_cloud: true,
          }));
        return null;
      }
      // 自动签到设置 + 今日台账（U-4 A1：签到页状态行 e2e 数据源，结构与 AutoCheckinStatusDto 同构）。
      if (cmd === "get_auto_checkin_settings") {
        return {
          enabled: true,
          daily_time_hhmm: "10:00",
          ledger: {
            date: new Date().toISOString().slice(0, 10),
            running: true,
            total: 3,
            completed: 1,
            failed: 0,
            skipped: 0,
          },
        };
      }
      if (cmd === "set_auto_checkin_settings") {
        // 回写后按传入值返回（设置页切换开关后的回读结果）。
        return {
          enabled: !!args?.enabled,
          daily_time_hhmm: String(args?.dailyTimeHhmm ?? "10:00"),
          ledger: null,
        };
      }
      if (cmd === "run_checkin") {
        if (scn.production) throw new Error("checkin_http_disabled");
        const ids = Array.isArray(args?.profileIds) ? args.profileIds : [];
        return {
          total: ids.length,
          completed: ids.length,
          failed: 0,
          cancelled: 0,
          results: ids.map((profileId: string) => ({
            profile_id: profileId,
            outcome: "claimed",
            state: "completed",
            claim_attempted: true,
            before: { enabled: true, checked_in: false, credits: 0, business_code: 0 },
            after: { enabled: true, checked_in: true, credits: 10, business_code: 0 },
            detail_code: null,
            started_at: "2025-07-01T00:00:00.000Z",
            finished_at: "2025-07-01T00:00:01.000Z",
          })),
        };
      }
      if (cmd === "cancel_checkin") return true;
      if (cmd === "refresh_managed_current_account") return MANAGED_ACCOUNT_STATE;
      if (cmd === "probe_source_key") return { ...KEY_STATUS, probe_state: "verified" };
      if (cmd === "register_source_key_candidate") {
        return {
          ...KEY_STATUS,
          source_key_pending_version: "work-cn-candidate-v1",
          source_key_activation_pending: true,
          probe_state: "verified_pending",
        };
      }
      // Tauri event 插件 command 只返回句柄，不产生真实事件；页面会继续用轮询兜底。
      if (cmd === "plugin:event|listen") return 0;
      if (cmd === "plugin:event|unlisten") return null;
      throw new Error(`mock bridge: 未模拟命令 ${cmd}`);
    }

    // 安装 Tauri internals mock——Tauri 2 invoke 通过此入口
    (window as any).__TAURI_INTERNALS__ = {
      invoke: mockInvoke,
      // 其他可能被 @tauri-apps/api 调用的入口
      convertFileSrc: (p: string) => p,
    };
  },
  { scenario },
);
}

/**
 * 在测试运行时切换场景（不重新加载页面）。
 */
export async function setScenario(page: Page, scenario: MockScenario) {
  await page.evaluate((scn) => {
    (window as any).__setScenario(scn);
  }, scenario);
}
