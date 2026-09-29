//! P11-6 Trae CN 签到只读探针（ADR-0032 决策 4：探针门禁，status 类查询先行）。
//!
//! 红线纪律：
//! - 只发 status 类**只读**查询；claim/sign 等任何会触发签到动作的端点
//!   一律不进入候选集合（`candidate_endpoints` 单测固化此约束）；
//! - 凭据只在内存解密使用，不落任何文件或日志；
//! - 报告只落形状摘要（顶层字段名、关注字段存在性、字节长度、SHA-256
//!   前 8 位指纹）与 HTTP/业务码，不落 token、设备号原值或完整响应原文。
//!
//! 候选端点从 Work CN 已知端点同构推导（`checkin_http` 的协议事实）：
//! 网关同为 `api.trae.cn`，候选路径取 status / 额度查询两类只读形态，
//! 另加 `GetUserInfo` 作为凭据可用性对照组。候选全 404/不可达同样是
//! 有效结论（Trae CN 无签到端点 → 前端永久显示 absent）。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::checkin_http::{API_BASE, GET_USER_INFO_PATH, TRAE_CN_IDE_VERSION};

/// Trae CN 探针使用的客户端标识头形态。
///
/// 与 Work 的遥测头集合（`apply_telemetry_headers`）分离：探针只带最小
/// 标准头（认证 + 设备 + UA/版本），不假借 Work SOLO 形态的固定遥测值。
const TRAE_CN_PROBE_USER_AGENT: &str = "VSCode 1.107.1 (Trae CN)";
/// Trae CN 安装包类型（与 OAuth client 同源的公开标识，非凭据）。
const TRAE_CN_PACKAGE_TYPE: &str = "stable_cn";

/// 候选端点（只读集合；`label` 用于报告与日志，`method`/`path` 为请求形态）。
const CANDIDATE_ENDPOINTS: &[(&str, &str, &str)] = &[
    // 对照组：凭据可用性（Work 已实证的只读身份查询，同构复用）。
    (
        "get_user_info_control",
        "POST",
        "/cloudide/api/v3/trae/GetUserInfo",
    ),
    // 候选 1：Work 签到状态查询的同构路径（ Trae CN 凭据 + Trae CN 设备）。
    (
        "checkin_status_post",
        "POST",
        "/trae/api/v2/ug/checkin_credits/status",
    ),
    // 候选 2：同路径 GET 形态（HTTP 语义天然只读；405 亦有分辨价值）。
    (
        "checkin_status_get",
        "GET",
        "/trae/api/v2/ug/checkin_credits/status",
    ),
    // 候选 3：Work 额度查询的同构路径（签到积分的上游只读查询）。
    (
        "ent_usage_post",
        "POST",
        "/trae/api/v2/pay/ide_user_ent_usage",
    ),
];

/// 单端点探测结果（脱敏：不含 token、设备号原值、响应原文）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TraeCnProbeFinding {
    /// 端点标识（"POST /trae/api/v2/ug/checkin_credits/status" 形态）。
    pub endpoint: String,
    /// ok / http_error / network_error / protocol_error。
    pub outcome: String,
    /// HTTP 状态码（网络层失败时为 None）。
    pub http_status: Option<u16>,
    /// 业务码（响应体 `code` 或 `ResponseMetadata.Error.Code`）。
    pub business_code: Option<i64>,
    /// JSON 顶层字段名清单（只取 key，不取 value）。
    pub body_top_level_keys: Vec<String>,
    /// 签到语义关注字段的存在性（checked_in / credits / enable / data / message）。
    pub body_field_presence: Vec<String>,
    /// 响应体 SHA-256 前 8 位指纹（同文同哈希，反查不出原文）。
    pub body_sha256_8: String,
    /// 响应体字节数。
    pub body_byte_len: usize,
}

/// 探针报告（落档形态；全部字段可安全入档）。
#[derive(Debug, Clone, Serialize)]
pub struct TraeCnCheckinProbeReport {
    pub probe: &'static str,
    /// 红线声明（随报告固化，供审阅时直接核对）。
    pub red_line: &'static str,
    pub credential_available: bool,
    pub accounts_probed: usize,
    pub findings: Vec<TraeCnProbeFinding>,
    pub conclusion: String,
    pub generated_at_unix_seconds: u64,
}

/// 候选端点清单（只读约束的单一事实源；单测固化「无 claim/sign」）。
pub fn candidate_endpoints() -> Vec<(&'static str, &'static str, &'static str)> {
    CANDIDATE_ENDPOINTS.to_vec()
}

/// 响应体形状摘要：顶层字段名 + 关注字段存在性 + 哈希 + 长度。
///
/// 只提取结构信息，任何字段值都不进入结果（防正文夹带凭据/原文外泄）。
fn shape_summary(body: &str) -> (Vec<String>, Vec<String>, String, usize) {
    let byte_len = body.as_bytes().len();
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    let hash = hex::encode(&hasher.finalize()[..4]);

    let focus_fields = ["checked_in", "credits", "enable", "data", "message"];
    let (keys, presence) = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(value) => {
            let keys = value
                .as_object()
                .map(|object| {
                    object
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let presence = focus_fields
                .iter()
                .filter(|field| value.get(**field).is_some())
                .map(|field| field.to_string())
                .collect();
            (keys, presence)
        }
        Err(_) => (Vec::new(), Vec::new()),
    };
    (keys, presence, hash, byte_len)
}

/// 从响应体提取业务码：`code` 字段优先（数字/字符串双形态兼容），
/// 其次 `ResponseMetadata.Error.Code`（ExchangeToken 信封口径）。
fn extract_business_code(body: &str) -> Option<i64> {
    let value = serde_json::from_str::<serde_json::Value>(body).ok()?;
    let code = value
        .get("code")
        .or_else(|| {
            value
                .get("ResponseMetadata")
                .and_then(|metadata| metadata.get("Error"))
                .and_then(|error| error.get("Code"))
        })?;
    code.as_i64()
        .or_else(|| code.as_str().and_then(|text| text.parse().ok()))
}

/// 对候选端点逐一发送只读请求并记录形状摘要。
///
/// 请求形态与 Work status 同构：POST 带 `{}` 体 + GET 无体；头集合为
/// 最小标准形态（Authorization / x-device-id / UA / 版本），遥测扩展头
/// 不假借 Work 的 SOLO 固定值。
pub fn probe_trae_cn_status_endpoints(
    client: &Client,
    access_token: &str,
    device_id: &str,
) -> Vec<TraeCnProbeFinding> {
    CANDIDATE_ENDPOINTS
        .iter()
        .map(|(label, method, path)| {
            probe_one(client, label, method, path, access_token, device_id)
        })
        .collect()
}

/// 单端点探测：发送请求 → 分类结果 → 形状摘要（任何失败不 panic）。
fn probe_one(
    client: &Client,
    _label: &str,
    method: &str,
    path: &str,
    access_token: &str,
    device_id: &str,
) -> TraeCnProbeFinding {
    let url = format!("{API_BASE}{path}");
    let request = client
        .request(
            reqwest::Method::from_bytes(method.as_bytes())
                .unwrap_or(reqwest::Method::GET),
            &url,
        )
        .header("Authorization", format!("Cloud-IDE-JWT {access_token}"))
        .header("x-device-id", device_id)
        .header("User-Agent", TRAE_CN_PROBE_USER_AGENT)
        .header("app-version", TRAE_CN_IDE_VERSION)
        .header("package-type", TRAE_CN_PACKAGE_TYPE)
        .header("x-user-region", "CN")
        .header("Content-Type", "application/json");

    let outcome = |finding: TraeCnProbeFinding| TraeCnProbeFinding {
        endpoint: format!("{method} {path}"),
        ..finding
    };
    let sent = if method == "POST" {
        request.body("{}").send()
    } else {
        request.send()
    };
    let mut finding = match sent {
        Err(_) => TraeCnProbeFinding {
            endpoint: String::new(),
            outcome: "network_error".to_string(),
            http_status: None,
            business_code: None,
            body_top_level_keys: Vec::new(),
            body_field_presence: Vec::new(),
            body_sha256_8: String::new(),
            body_byte_len: 0,
        },
        Ok(response) => {
            let status = response.status().as_u16();
            let body_text = response.text().unwrap_or_default();
            let (keys, presence, hash, len) = shape_summary(&body_text);
            let business_code = extract_business_code(&body_text);
            TraeCnProbeFinding {
                endpoint: String::new(),
                outcome: if response_status_is_success(status) {
                    "ok".to_string()
                } else {
                    "http_error".to_string()
                },
                http_status: Some(status),
                business_code,
                body_top_level_keys: keys,
                body_field_presence: presence,
                body_sha256_8: hash,
                body_byte_len: len,
            }
        }
    };
    if finding.outcome == "ok" && finding.body_top_level_keys.is_empty() && finding.body_byte_len > 0
    {
        // 2xx 但响应体不是 JSON：协议形态与 Work 口径不符（如网关 HTML）。
        finding.outcome = "protocol_error".to_string();
    }
    outcome(finding)
}

fn response_status_is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// GetUserInfo 健康检测形状取证（真机验收：原始响应形状记录）。
///
/// 请求形状与 `get_user_info_full_with_request` 的 TraeCn 分支逐字同源：
/// 同端点常量（`GET_USER_INFO_PATH`）、同认证头（仅 `x-cloudide-token`）、
/// 同体（`ReqSource=IDE` + `TRAE_CN_IDE_VERSION`），不带签到遥测头集合——
/// 这是 `check_trae_cn_account_health_inner` 的真实请求形状，与签到探针
/// 对照组（Authorization + 遥测头）形状不同。只读幂等查询；结果只含
/// HTTP 状态码、业务码、形状摘要与哈希，不含 token 或响应原文。
pub fn probe_get_user_info_shape(client: &Client, access_token: &str) -> TraeCnProbeFinding {
    let url = format!("{API_BASE}{GET_USER_INFO_PATH}");
    let body = serde_json::json!({
        "ReqSource": "IDE",
        "IDEVersion": TRAE_CN_IDE_VERSION,
    });
    let sent = client
        .post(&url)
        .header("x-cloudide-token", access_token)
        .json(&body)
        .send();
    let mut finding = match sent {
        Err(_) => TraeCnProbeFinding {
            endpoint: format!("POST {GET_USER_INFO_PATH}"),
            outcome: "network_error".to_string(),
            http_status: None,
            business_code: None,
            body_top_level_keys: Vec::new(),
            body_field_presence: Vec::new(),
            body_sha256_8: String::new(),
            body_byte_len: 0,
        },
        Ok(response) => {
            let status = response.status().as_u16();
            let body_text = response.text().unwrap_or_default();
            let (keys, presence, hash, len) = shape_summary(&body_text);
            TraeCnProbeFinding {
                endpoint: format!("POST {GET_USER_INFO_PATH}"),
                outcome: if response_status_is_success(status) {
                    "ok".to_string()
                } else {
                    "http_error".to_string()
                },
                http_status: Some(status),
                business_code: extract_business_code(&body_text),
                body_top_level_keys: keys,
                body_field_presence: presence,
                body_sha256_8: hash,
                body_byte_len: len,
            }
        }
    };
    if finding.outcome == "ok" && finding.body_top_level_keys.is_empty() && finding.body_byte_len > 0
    {
        // 2xx 但响应体不是 JSON：协议形态与 GetUserInfo 信封口径不符。
        finding.outcome = "protocol_error".to_string();
    }
    finding
}

/// 从探测结果推导结论（纯函数；落档与单测共用）。
///
/// 判定顺序（2026-09-28 实测校准）：签到状态端点的 `code=0` 是业务成功
/// ——token 无效时网关返回 401/业务拒绝码，不会返回 code=0——因此它是
/// 「端点存在且凭据可用」的直接证据，优先于对照组；GetUserInfo 对照组
/// 失败仅作附注（该 cloudide 路径对 Trae CN token 形态的适用性存疑，
/// 见探针报告 401/20310 实测）。对照组通过且候选全 404 → 无端点结论。
pub fn derive_probe_conclusion(findings: &[TraeCnProbeFinding]) -> String {
    let control = findings
        .iter()
        .find(|finding| finding.endpoint.contains("GetUserInfo"));
    let status_like = findings.iter().find(|finding| {
        finding.endpoint.contains("checkin_credits/status") && finding.outcome == "ok"
    });
    if let Some(finding) = status_like {
        if finding.business_code == Some(0) {
            let mut conclusion =
                "签到状态端点存在且可用（code=0；checked_in/credits 字段同构 Work）".to_string();
            let control_ok = control.is_some_and(|finding| {
                finding.outcome == "ok" && finding.business_code.map_or(true, |code| code == 0)
            });
            if !control_ok {
                let code = control
                    .and_then(|finding| finding.business_code)
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "缺失".to_string());
                conclusion.push_str(&format!(
                    "；附注：GetUserInfo 对照组未通过（业务码 {code}）——该路径对 Trae CN token 形态的适用性存疑，不影响签到端点结论"
                ));
            }
            return conclusion;
        }
    }

    let credential_ok = control.is_some_and(|finding| {
        finding.outcome == "ok" && finding.business_code.map_or(true, |code| code == 0)
    });
    if !credential_ok {
        let reason = control
            .map(|finding| match finding.outcome.as_str() {
                "network_error" => "网络不可达",
                "http_error" => "token 被拒或身份查询失败",
                _ => "响应形态异常",
            })
            .unwrap_or("对照组缺失");
        return format!("凭据不可用（{reason}），候选端点结论不可信");
    }

    let candidates: Vec<&TraeCnProbeFinding> = findings
        .iter()
        .filter(|finding| !finding.endpoint.contains("GetUserInfo"))
        .collect();
    let reachable = candidates
        .iter()
        .any(|finding| finding.outcome == "ok" && finding.http_status != Some(404));
    if !reachable {
        return "候选端点全部 404/不可达：Trae CN 无签到端点（前端显示 absent）".to_string();
    }
    match status_like {
        Some(finding) => match finding.business_code {
            Some(code) => format!("签到状态端点存在，业务码 {code}（需人工判读）"),
            None => "签到状态端点存在，响应无业务码字段（需人工判读）".to_string(),
        },
        None => "签到状态路径不可用，但额度类候选可达（需人工判读）".to_string(),
    }
}

/// 生成报告（凭据可用性、账号数、结论由调用方给出；findings 为探测结果）。
pub fn build_probe_report(
    credential_available: bool,
    accounts_probed: usize,
    findings: Vec<TraeCnProbeFinding>,
    conclusion: String,
) -> TraeCnCheckinProbeReport {
    TraeCnCheckinProbeReport {
        probe: "trae-cn-checkin-status-probe",
        red_line: "只读探针：未发送 claim/sign；仅 status 类查询；报告不含 token/原文",
        credential_available,
        accounts_probed,
        findings,
        conclusion,
        generated_at_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
    }
}

/// 报告落档：`{reports_dir}/trae-cn-checkin-probe-{YYYYMMDD}.json`。
///
/// 目录不存在时创建；写失败返回错误由调用方决定降级（打印不落档）。
pub fn write_probe_report(
    reports_dir: &Path,
    report: &TraeCnCheckinProbeReport,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(reports_dir).map_err(|_| "probe_report_dir_unavailable".to_string())?;
    let date = yyyymmdd(report.generated_at_unix_seconds);
    let path = reports_dir.join(format!("trae-cn-checkin-probe-{date}.json"));
    let content = serde_json::to_vec_pretty(report)
        .map_err(|_| "probe_report_serialize_failed".to_string())?;
    std::fs::write(&path, content).map_err(|_| "probe_report_write_failed".to_string())?;
    Ok(path)
}

/// Unix 秒 → `YYYYMMDD`（UTC；落档日期仅作归档定位，不作精确时刻证据）。
fn yyyymmdd(unix_seconds: u64) -> String {
    // Civil-from-days（Howard Hinnant 算法；无需 chrono 依赖）。
    let days = (unix_seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}{m:02}{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 红线固化：候选集合中不得出现任何会触发签到动作的端点。
    #[test]
    fn candidate_endpoints_exclude_write_paths() {
        for (label, method, path) in candidate_endpoints() {
            let lowered = format!("{label} {method} {path}").to_ascii_lowercase();
            assert!(
                !lowered.contains("claim") && !lowered.contains("/sign"),
                "候选端点 {label} 混入写动作路径：{path}"
            );
        }
    }

    /// 形状摘要只提取结构：字段值不进入结果，哈希稳定可复现。
    #[test]
    fn shape_summary_exposes_structure_without_values() {
        let body = r#"{"code":0,"checked_in":true,"credits":42,"secret_token":"abc"}"#;
        let (keys, presence, hash, len) = shape_summary(body);
        assert!(keys.contains(&"code".to_string()));
        assert!(keys.contains(&"secret_token".to_string()));
        // 关注字段只记存在性，值（true/42）不落档。
        assert!(presence.contains(&"checked_in".to_string()));
        assert!(presence.contains(&"credits".to_string()));
        assert!(!serde_json::to_string(&presence).unwrap().contains("42"));
        assert_eq!(len, body.len());
        // 同文同哈希。
        assert_eq!(shape_summary(body).2, hash);
    }

    /// 非 JSON 响应体：形状摘要为空结构（长度与哈希仍记录）。
    #[test]
    fn shape_summary_handles_non_json_body() {
        let (keys, presence, _hash, len) = shape_summary("<html>gateway</html>");
        assert!(keys.is_empty());
        assert!(presence.is_empty());
        assert_eq!(len, "<html>gateway</html>".len());
    }

    /// 业务码提取：数字与字符串双形态 + 信封口径回退。
    #[test]
    fn extract_business_code_handles_number_string_and_envelope() {
        assert_eq!(extract_business_code(r#"{"code":9095}"#), Some(9095));
        assert_eq!(extract_business_code(r#"{"code":"20401"}"#), Some(20401));
        assert_eq!(
            extract_business_code(
                r#"{"ResponseMetadata":{"Error":{"Code":"20324"}}}"#
            ),
            Some(20324)
        );
        assert_eq!(extract_business_code(r#"{"other":1}"#), None);
    }

    /// 结论推导：对照组失败 → 凭据不可用；候选全 404 → 无端点；可用 → 可用。
    #[test]
    fn derive_probe_conclusion_covers_control_not_found_and_available() {
        let control_fail = TraeCnProbeFinding {
            endpoint: "POST /cloudide/api/v3/trae/GetUserInfo".to_string(),
            outcome: "http_error".to_string(),
            http_status: Some(401),
            business_code: None,
            body_top_level_keys: Vec::new(),
            body_field_presence: Vec::new(),
            body_sha256_8: String::new(),
            body_byte_len: 0,
        };
        assert!(derive_probe_conclusion(&[control_fail]).contains("凭据不可用"));

        let not_found = TraeCnProbeFinding {
            endpoint: "POST /trae/api/v2/ug/checkin_credits/status".to_string(),
            outcome: "http_error".to_string(),
            http_status: Some(404),
            business_code: None,
            body_top_level_keys: Vec::new(),
            body_field_presence: Vec::new(),
            body_sha256_8: String::new(),
            body_byte_len: 0,
        };
        let control_ok = TraeCnProbeFinding {
            endpoint: "POST /cloudide/api/v3/trae/GetUserInfo".to_string(),
            outcome: "ok".to_string(),
            http_status: Some(200),
            business_code: Some(0),
            body_top_level_keys: vec!["code".to_string()],
            body_field_presence: Vec::new(),
            body_sha256_8: String::new(),
            body_byte_len: 2,
        };
        assert!(derive_probe_conclusion(&[control_ok.clone(), not_found]).contains("无签到端点"));

        let available = TraeCnProbeFinding {
            endpoint: "POST /trae/api/v2/ug/checkin_credits/status".to_string(),
            outcome: "ok".to_string(),
            http_status: Some(200),
            business_code: Some(0),
            body_top_level_keys: vec!["code".to_string(), "checked_in".to_string()],
            body_field_presence: vec!["checked_in".to_string()],
            body_sha256_8: String::new(),
            body_byte_len: 20,
        };
        assert!(derive_probe_conclusion(&[control_ok, available.clone()]).contains("端点存在且可用"));

        // 2026-09-28 实测形态固化：status code=0 是「端点存在且凭据可用」
        // 的直接证据，对照组（GetUserInfo 401/20310）失败仅作附注不否决。
        let control_rejected = TraeCnProbeFinding {
            endpoint: "POST /cloudide/api/v3/trae/GetUserInfo".to_string(),
            outcome: "http_error".to_string(),
            http_status: Some(401),
            business_code: Some(20310),
            body_top_level_keys: Vec::new(),
            body_field_presence: Vec::new(),
            body_sha256_8: String::new(),
            body_byte_len: 264,
        };
        let rejected_conclusion = derive_probe_conclusion(&[control_rejected, available]);
        assert!(rejected_conclusion.contains("端点存在且可用"));
        assert!(rejected_conclusion.contains("20310"));
    }

    /// 报告落档：按日命名 + 内容可回读 + 不含传入的敏感材料样例。
    #[test]
    fn write_probe_report_uses_daily_filename_and_stays_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let report = build_probe_report(
            true,
            1,
            vec![TraeCnProbeFinding {
                endpoint: "POST /trae/api/v2/ug/checkin_credits/status".to_string(),
                outcome: "ok".to_string(),
                http_status: Some(200),
                business_code: Some(0),
                body_top_level_keys: vec!["code".to_string()],
                body_field_presence: vec!["checked_in".to_string()],
                body_sha256_8: "abcdef12".to_string(),
                body_byte_len: 20,
            }],
            "签到状态端点存在且可用（code=0）".to_string(),
        );
        let path = write_probe_report(dir.path(), &report).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("trae-cn-checkin-probe-"));
        assert!(name.ends_with(".json"));
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("trae-cn-checkin-status-probe"));
        // 红线声明随报告固化（审阅时直接可核对）。
        assert!(content.contains("未发送 claim/sign"));
    }

    /// 落档日期换算：固定 Unix 秒 → YYYYMMDD（2026-09-28 = 1790+... 已知锚点）。
    #[test]
    fn yyyymmdd_converts_known_anchor() {
        // 2026-09-28 00:00:00 UTC = 1790572800。
        assert_eq!(yyyymmdd(1_790_572_800), "20260928");
        assert_eq!(yyyymmdd(0), "19700101");
    }
}
