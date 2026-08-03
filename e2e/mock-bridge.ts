// ============================================================================
// T03/T04 Playwright mock 命令边界
// ============================================================================
//
// 关键约束（交接文档 Playwright 章节）：
// - 使用确定性 mock 命令边界，绝不启动 Tauri 或访问真实 TRAE 数据
// - 通过 page.addInitScript 在应用加载前安装 window.__TAURI_INTERNALS__.invoke
// - 所有返回数据为合成 fixture，不含真实账号/会话/认证正文
//
// Tauri 2 中 @tauri-apps/api/core 的 invoke 最终调用
// window.__TAURI_INTERNALS__.invoke(cmd, args, options)，因此 mock 该入口即可。

import type { Page } from "@playwright/test";

// 合成工作台状态：T01 空状态，但 scan_enabled=true 以便历史库自身授权控制
export const MOCK_WORKSPACE_STATE = {
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
  history: {
    account_count: 0,
    project_count: 0,
    session_count: 0,
  },
  capabilities: {
    scan_enabled: true,
    sync_enabled: false,
    backup_enabled: false,
    restore_enabled: false,
  },
  honest_status: "真实能力尚未启用",
};

// 合成扫描成功结果
export const MOCK_SCAN_SUCCESS = {
  kind: "success",
  snapshot_id: "snap-fixture-1",
  snapshot_meta: {
    snapshot_id: "snap-fixture-1",
    platform_id: "work_cn",
    data_location_id: "loc-fixture",
    product_version: "1.0.0-fixture",
    schema_fingerprint: "fp-fixture-abc",
    mapping_version: "work_cn_v1",
    account_evidence_ref: null,
    captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
    files: [],
    fingerprint: "fp-fixture-abc",
  },
  catalog_updated: true,
};

// 合成浏览结果：两账号、两项目、两会话
export const MOCK_BROWSE_RESULT = {
  accounts: [
    {
      user_id: "user-A",
      display_label: "User A",
      project_count: 1,
      session_count: 2,
    },
    {
      user_id: "user-B",
      display_label: "User B",
      project_count: 1,
      session_count: 0,
    },
  ],
  projects: [
    {
      project_id: "p1",
      display_name: "Project 阿尔法",
      display_owner: "user-A",
      session_count: 2,
    },
    {
      project_id: "p2",
      display_name: "Project 贝塔",
      display_owner: "user-B",
      session_count: 0,
    },
  ],
  sessions: [
    {
      session_identity: {
        product_history_namespace: "work_cn",
        original_session_id: "session-aaa",
      },
      title: "会话 AAA 关于如何构建稳定的历史库",
      message_count: 2,
      last_captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
      project_id: "p1",
    },
    {
      session_identity: {
        product_history_namespace: "work_cn",
        original_session_id: "session-bbb",
      },
      title: "会话 BBB 软删除与版本图",
      message_count: 1,
      last_captured_at: { secs_since_epoch: 1700000001, nanos_since_epoch: 0 },
      project_id: "p1",
    },
  ],
  summary: {
    visible_account_count: 2,
    visible_project_count: 2,
    visible_session_count: 2,
    soft_deleted_project_count: 0,
    soft_deleted_session_count: 0,
    soft_deleted_message_count: 0,
  },
};

// 空浏览结果
export const MOCK_EMPTY_BROWSE = {
  accounts: [],
  projects: [],
  sessions: [],
  summary: {
    visible_account_count: 0,
    visible_project_count: 0,
    visible_session_count: 0,
    soft_deleted_project_count: 0,
    soft_deleted_session_count: 0,
    soft_deleted_message_count: 0,
  },
};

// 合成对话预览
export function makePreview(sessionId: string) {
  if (sessionId === "session-aaa") {
    return {
      session_identity: {
        product_history_namespace: "work_cn",
        original_session_id: sessionId,
      },
      title: "会话 AAA 关于如何构建稳定的历史库",
      messages: [
        {
          message_id: "m1",
          session_id: sessionId,
          role: "user",
          content_excerpt: "hello world 如何构建稳定的历史库",
          soft_deleted: false,
          seq: 0,
        },
        {
          message_id: "m2",
          session_id: sessionId,
          role: "assistant",
          content_excerpt: "使用确定性内容图与软删除语义",
          soft_deleted: false,
          seq: 1,
        },
      ],
      total_message_count: 2,
    };
  }
  return {
    session_identity: {
      product_history_namespace: "work_cn",
      original_session_id: sessionId,
    },
    title: "会话 BBB 软删除与版本图",
    messages: [
      {
        message_id: "m3",
        session_id: sessionId,
        role: "user",
        content_excerpt: "软删除内容是否保留证据",
        soft_deleted: false,
        seq: 0,
      },
    ],
    total_message_count: 1,
  };
}

// 合成搜索命中
export const MOCK_SEARCH_HITS = [
  {
    session_identity: {
      product_history_namespace: "work_cn",
      original_session_id: "session-aaa",
    },
    message_id: "m1",
    project_id: "p1",
    title: "会话 AAA 关于如何构建稳定的历史库",
    content_excerpt: "hello world 如何构建稳定的历史库",
    role: "user",
  },
];

// 扫描失败原因类型
export type ScanFailureReason =
  | "not_authorized"
  | "process_running"
  | "database_missing"
  | "schema_incompatible"
  | "storage_root_unavailable"
  | "source_set_drift"
  | "catalog_transaction_failed"
  | "catalog_key_missing";

// 场景配置：每个场景决定 scan_history / browse_history 等命令的返回
export interface MockScenario {
  // scan_history 返回：success | failed | deduplicated
  scanOutcome?: "success" | "deduplicated" | { failed: ScanFailureReason };
  // browse_history 返回：full | empty | error
  browseMode?: "full" | "empty" | "error";
  // search_history 返回：hits | empty | error
  searchMode?: "hits" | "empty" | "error";
  // read_work_cn_state 默认返回 missing（不自动调用）
}

/**
 * 在页面加载前安装 mock invoke。
 * 测试通过 setScenario 切换场景，避免每个测试重新加载页面。
 */
export async function installMockBridge(
  page: Page,
  scenario: MockScenario = {},
) {
  // 把场景序列化注入页面，作为初始 active scenario
  await page.addInitScript((scn) => {
    // 场景容器——测试运行时可通过 window.__setScenario 更新
    (window as any).__activeScenario = scn;
    (window as any).__setScenario = (s: unknown) => {
      (window as any).__activeScenario = s;
    };

    // 预定义合成数据
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

    const SCAN_SUCCESS = {
      kind: "success",
      snapshot_id: "snap-fixture-1",
      snapshot_meta: {
        snapshot_id: "snap-fixture-1",
        platform_id: "work_cn",
        data_location_id: "loc-fixture",
        product_version: "1.0.0-fixture",
        schema_fingerprint: "fp-fixture-abc",
        mapping_version: "work_cn_v1",
        account_evidence_ref: null,
        captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
        files: [],
        fingerprint: "fp-fixture-abc",
      },
      catalog_updated: true,
    };

    const BROWSE_FULL = {
      accounts: [
        {
          user_id: "user-A",
          display_label: "User A",
          project_count: 1,
          session_count: 2,
        },
        {
          user_id: "user-B",
          display_label: "User B",
          project_count: 1,
          session_count: 0,
        },
      ],
      projects: [
        {
          project_id: "p1",
          display_name: "Project 阿尔法",
          display_owner: "user-A",
          session_count: 2,
        },
        {
          project_id: "p2",
          display_name: "Project 贝塔",
          display_owner: "user-B",
          session_count: 0,
        },
      ],
      sessions: [
        {
          session_identity: {
            product_history_namespace: "work_cn",
            original_session_id: "session-aaa",
          },
          title: "会话 AAA 关于如何构建稳定的历史库",
          message_count: 2,
          last_captured_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
          project_id: "p1",
        },
        {
          session_identity: {
            product_history_namespace: "work_cn",
            original_session_id: "session-bbb",
          },
          title: "会话 BBB 软删除与版本图",
          message_count: 1,
          last_captured_at: { secs_since_epoch: 1700000001, nanos_since_epoch: 0 },
          project_id: "p1",
        },
      ],
      summary: {
        visible_account_count: 2,
        visible_project_count: 2,
        visible_session_count: 2,
        soft_deleted_project_count: 0,
        soft_deleted_session_count: 0,
        soft_deleted_message_count: 0,
      },
    };

    const BROWSE_EMPTY = {
      accounts: [],
      projects: [],
      sessions: [],
      summary: {
        visible_account_count: 0,
        visible_project_count: 0,
        visible_session_count: 0,
        soft_deleted_project_count: 0,
        soft_deleted_session_count: 0,
        soft_deleted_message_count: 0,
      },
    };

    const SEARCH_HITS = [
      {
        session_identity: {
          product_history_namespace: "work_cn",
          original_session_id: "session-aaa",
        },
        message_id: "m1",
        project_id: "p1",
        title: "会话 AAA 关于如何构建稳定的历史库",
        content_excerpt: "hello world 如何构建稳定的历史库",
        role: "user",
      },
    ];

    const SYNC_PLAN = {
      operation_id: "op-fixture-plan",
      current_user_id: "user-B",
      scope_snapshot: { kind: "all_history" },
      actions: [
        {
          kind: "attach_sessions",
          source_project_id: "p1",
          target_project_id: "p2",
          session_ids: [
            {
              product_history_namespace: "work_cn",
              original_session_id: "session-aaa",
            },
          ],
        },
      ],
      exclusions: [
        {
          project_id: "p1",
          session_id: {
            product_history_namespace: "work_cn",
            original_session_id: "session-bbb",
          },
          reason: "already_current",
        },
      ],
    };

    function makePreview(sessionId: string) {
      if (sessionId === "session-aaa") {
        return {
          session_identity: {
            product_history_namespace: "work_cn",
            original_session_id: sessionId,
          },
          title: "会话 AAA 关于如何构建稳定的历史库",
          messages: [
            {
              message_id: "m1",
              session_id: sessionId,
              role: "user",
              content_excerpt: "hello world 如何构建稳定的历史库",
              soft_deleted: false,
              seq: 0,
            },
            {
              message_id: "m2",
              session_id: sessionId,
              role: "assistant",
              content_excerpt: "使用确定性内容图与软删除语义",
              soft_deleted: false,
              seq: 1,
            },
          ],
          total_message_count: 2,
        };
      }
      return {
        session_identity: {
          product_history_namespace: "work_cn",
          original_session_id: sessionId,
        },
        title: "会话 BBB 软删除与版本图",
        messages: [
          {
            message_id: "m3",
            session_id: sessionId,
            role: "user",
            content_excerpt: "软删除内容是否保留证据",
            soft_deleted: false,
            seq: 0,
          },
        ],
        total_message_count: 1,
      };
    }

    // R7：mock 授权状态机——模拟后端 AuthorizationState
    // 未授权时 scan_history 必须返回 failed/not_authorized，不能无条件返回成功
    const authState: {
      status: "not_authorized" | "authorized";
      canonicalFixtureRoot: string | null;
      dbRelativePath: string | null;
    } = { status: "not_authorized", canonicalFixtureRoot: null, dbRelativePath: null };

    // mock invoke 实现：根据 activeScenario 返回合成数据
    async function mockInvoke(cmd: string, args?: any) {
      const scn = (window as any).__activeScenario || {};
      if (cmd === "get_workspace_state") return WS;
      if (cmd === "read_work_cn_state") {
        // T02 入口默认返回 missing 状态——不自动调用
        return {
          platform: WS.platform,
          data_location: {
            selected: true,
            display_name: "C:\\fixture",
            unavailable_reason: null,
          },
          compatibility: { kind: "Missing" },
          current_account: {
            user_id: null,
            source_events: [],
            auth_fingerprint: null,
            local_storage_user_id: null,
            product_version: null,
            observed_at: { secs_since_epoch: 1700000000, nanos_since_epoch: 0 },
            evidence_state: "missing",
          },
          readonly_reason: "missing",
        };
      }
      // R7：grant_scan_authorization 建立后端授权状态
      if (cmd === "grant_scan_authorization") {
        const fr = args?.fixtureRoot;
        const db = args?.dbRelativePath;
        if (!fr || !db) {
          throw new Error("fixture_root 或 db_relative_path 不能为空");
        }
        // 模拟后端 canonicalize——直接返回原路径作为 canonical
        const canonical = String(fr);
        authState.status = "authorized";
        authState.canonicalFixtureRoot = canonical;
        authState.dbRelativePath = String(db);
        return canonical;
      }
      // R7：revoke_scan_authorization 撤销后端授权
      if (cmd === "revoke_scan_authorization") {
        authState.status = "not_authorized";
        authState.canonicalFixtureRoot = null;
        authState.dbRelativePath = null;
        return null;
      }
      if (cmd === "scan_history") {
        // R7：未授权时必须返回 failed/not_authorized——不能无条件返回成功
        if (authState.status !== "authorized") {
          return { kind: "failed", reason: "not_authorized" };
        }
        // R7：授权范围不匹配——返回 failed/not_authorized
        const requestedFr = args?.fixtureRoot;
        const requestedDb = args?.dbRelativePath;
        if (
          requestedFr !== authState.canonicalFixtureRoot ||
          requestedDb !== authState.dbRelativePath
        ) {
          return { kind: "failed", reason: "not_authorized" };
        }
        // 进程运行中——返回 failed/process_running
        if (args?.processState === "running") {
          return { kind: "failed", reason: "process_running" };
        }
        const outcome = scn.scanOutcome ?? "success";
        if (outcome === "success") return SCAN_SUCCESS;
        if (outcome === "deduplicated") {
          return {
            kind: "deduplicated",
            existing_snapshot_id: "snap-fixture-1",
            fingerprint: "fp-fixture-abc",
          };
        }
        // failed
        return { kind: "failed", reason: outcome.failed };
      }
      if (cmd === "browse_history") {
        const mode = scn.browseMode ?? "full";
        if (mode === "error") throw new Error("mock browse error");
        return mode === "empty" ? BROWSE_EMPTY : BROWSE_FULL;
      }
      if (cmd === "search_history") {
        const mode = scn.searchMode ?? "hits";
        if (mode === "error") throw new Error("mock search error");
        return mode === "empty" ? [] : SEARCH_HITS;
      }
      if (cmd === "read_conversation") {
        const sid = args?.session?.original_session_id;
        if (!sid) return null;
        return makePreview(sid);
      }
      if (cmd === "assign_source") return true;
      if (cmd === "build_sync_plan") {
        if (authState.status !== "authorized") {
          throw new Error("未授权扫描");
        }
        return { ...SYNC_PLAN, scope_snapshot: args?.scope ?? SYNC_PLAN.scope_snapshot };
      }
      throw new Error(`mock bridge: 未模拟命令 ${cmd}`);
    }

    // 安装 Tauri internals mock——Tauri 2 invoke 通过此入口
    (window as any).__TAURI_INTERNALS__ = {
      invoke: mockInvoke,
      // 其他可能被 @tauri-apps/api 调用的入口
      convertFileSrc: (p: string) => p,
    };
  }, scenario);
}

/**
 * 在测试运行时切换场景（不重新加载页面）。
 */
export async function setScenario(page: Page, scenario: MockScenario) {
  await page.evaluate((scn) => {
    (window as any).__setScenario(scn);
  }, scenario);
}
