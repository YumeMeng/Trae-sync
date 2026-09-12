import { describe, expect, it } from "vitest";
import {
  isReadAuthorizationInvalidationError,
  safeUiErrorMessage,
  safeUiRecommendedAction,
} from "../src/utils/safeUiError";

describe("读取授权失效错误映射", () => {
  it.each([
    "未授权扫描：后端未持有显式用户授权",
    "授权不匹配：请求范围与授权范围不一致",
    "TRAE 默认根目录不存在",
    "data_location_unavailable: TRAE 默认数据库不存在",
  ])("识别后端返回的中文授权错误：%s", (errorText) => {
    expect(isReadAuthorizationInvalidationError(errorText)).toBe(true);
    expect(safeUiErrorMessage(errorText, "fallback")).toContain("重新授权");
  });

  it("同时检查结构化错误的 code 和 message", () => {
    const error = {
      code: "command_error",
      message: "未授权扫描：后端未持有显式用户授权",
    };

    expect(isReadAuthorizationInvalidationError(error)).toBe(true);
    expect(safeUiErrorMessage(error, "fallback")).toContain("重新授权");
  });

  it("目录库写协议升级错误只显示固定提示和处理动作", () => {
    const error = {
      code: "catalog_write_protocol_upgrade_required",
      message: String.raw`D:\private\catalog.db raw-key-secret`,
    };

    expect(safeUiErrorMessage(error, "fallback")).toBe(
      "当前目录库采用了此版本不支持的写入协议，未写入新的历史或修改目录库。",
    );
    expect(safeUiRecommendedAction(error, "fallback")).toBe(
      "保留目录库及其 sidecar；不要手动删除 WAL/SHM，等待后续目录库旁路升级功能。",
    );
  });

  it.each([
    ["account_profile_store_busy", "另一实例正在更新账号档案，请稍后重试。"],
    ["account_fingerprint_salt_unavailable", "账号安全摘要暂不可用，请检查本地存储权限后重试。"],
    [
      "account_profile_store_salt_mismatch",
      "账号档案与当前安装不匹配，已停止读取；请保留现有档案后重试。",
    ],
    [
      "account_profile_store_salt_binding_required",
      "账号档案需要在当前安装中重新绑定，现有记录未被覆盖。",
    ],
    ["account_profile_store_invalid", "账号档案校验失败，已停止读取；请保留现有档案后重试。"],
    ["account_profile_store_path_invalid", "账号档案校验失败，已停止读取；请保留现有档案后重试。"],
    ["account_fingerprint_salt_invalid", "账号档案校验失败，已停止读取；请保留现有档案后重试。"],
  ])("账号档案错误 %s 使用稳定脱敏提示", (code, message) => {
    const error = { code, message: String.raw`D:\private\account-profile.json secret` };
    expect(safeUiErrorMessage(error, "fallback")).toBe(message);
    expect(safeUiErrorMessage(error, "fallback")).not.toContain("private");
    expect(safeUiRecommendedAction(error, "fallback")).not.toContain("private");
  });

  it("签到能力未开放时使用明确提示", () => {
    expect(safeUiErrorMessage("checkin_http_disabled", "fallback")).toBe(
      "真实签到当前未启用；历史扫描和账号切换仍可正常使用。",
    );
    expect(safeUiRecommendedAction("checkin_http_disabled", "fallback")).toContain(
      "独立能力 Gate",
    );
  });

  it("主库当前账号未确认时不暴露内部错误码", () => {
    expect(safeUiErrorMessage("master_current_account_unavailable", "fallback")).toBe(
      "主库当前登录账号未确认，请先在环境页确认账号后重试。",
    );
  });

  it.each([
    ["master_archive_busy", "主库正在生成回复，归档设置需等它完成；稍后重试即可。"],
    ["master_archive_close_failed", "主库实例未能关闭，归档设置尚未写入；请关闭 TRAE 后重试。"],
    ["master_archive_conflict", "同一会话同时出现在归档和恢复设置中，请调整后再应用。"],
    ["master_archive_join_failed", "归档设置提交未完成，草稿仍保留；请稍后重试。"],
  ])("归档批次错误 %s 使用稳定提示", (code, message) => {
    expect(safeUiErrorMessage(code, "fallback")).toBe(message);
  });

  it("凭据维护状态不可用时显示可执行提示而不暴露错误码", () => {
    expect(safeUiErrorMessage("credential_maintenance_state_unavailable", "fallback")).toBe(
      "凭据维护状态暂时不可读取，本次未发起自动刷新；请检查本地存储后重试。",
    );
  });

  it.each([
    ["login_real_mode_required", "当前运行模式不支持账号登录。"],
    ["login_begin_failed", "登录会话创建失败，请重新发起登录。"],
    ["login_join_failed", "登录会话创建失败，请重新发起登录。"],
    ["login_not_started", "没有进行中的登录，请先点击添加账号。"],
    ["login_callback_invalid", "浏览器登录回调无效，请重新发起登录。"],
    [
      "login_exchange_failed",
      "登录凭证换取失败（服务端拒绝）；详细原因已记录在诊断日志（数据目录 checkin/login-diagnostics.log），可重新发起登录再试一次。",
    ],
    ["login_token_invalid", "服务端返回的登录凭证无效，请重新登录。"],
    ["login_storage_failed", "登录信息保存失败，请重试。"],
    ["checkin_storage_unavailable", "本机登录存储不可用，请检查磁盘后重试。"],
  ])("登录错误 %s 使用稳定脱敏提示", (code, message) => {
    // 后端登录失败以稳定错误码字符串返回，不含 Token 或回调正文。
    expect(safeUiErrorMessage(code, "fallback")).toBe(message);
  });
});
