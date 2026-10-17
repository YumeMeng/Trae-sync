//! P17-T3：产品描述符——跨产品差异点的统一声明来源（spec
//! docs/specs/p17-workbuddy-adaptation-research.md，用户决策"复用扩展不重写"）。
//!
//! fact-finder 盘点的五层散落中，存储根已由 [`crate::storage_roots`] 收口、
//! 防逃逸判定已由 [`crate::link_guard`] 收口；本模块收口剩余三层：
//! 客户端集成事实（进程/路径/本地 API）、凭据协议形态、用户身份形态。
//!
//! 约束（expand-contract 纪律）：本模块只**声明**差异点并供新代码消费；
//! 存量调用点的替换随 T4 双产品扩展渐进进行，不做机械大爆炸。

/// 客户端集成事实：进程匹配、数据根惯例与本地探针 API。
/// 全部字段来自 P17-T1/T2 探针实测（docs/specs/p17-workbuddy-adaptation-research.md）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIntegration {
    /// 进程映像名（taskkill/进程归属匹配；不含 .exe 后缀）。
    pub image_names: &'static [&'static str],
    /// 主程序文件名（launch 入口发现）。
    pub executable_names: &'static [&'static str],
    /// Electron `--user-data-dir` 惯例根（None = 未观察到显式惯例，
    /// 按 Electron 默认 `%APPDATA%/{productName}` 推导）。
    pub user_data_dir_override: Option<&'static str>,
    /// 本地探针 API 端口组（WorkBuddy：LocalProbeServer 18488/18489/18490）。
    pub local_probe_ports: &'static [u16],
    /// 本地探针 API 路径（如 WorkBuddy 的 `/workbuddy/probe`）。
    pub local_probe_path: Option<&'static str>,
}

impl ClientIntegration {
    /// 构造本地探针 URL（端口组内逐个尝试由调用方编排）。
    pub fn probe_url(&self, port: u16) -> Option<String> {
        self.local_probe_path
            .as_ref()
            .map(|path| format!("http://127.0.0.1:{port}{path}"))
    }
}

/// 用户身份形态差异（跨产品不可假定同构）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserIdShape {
    /// 平台用户 ID 的十进制位数（Trae 16 位 / WorkBuddy 20 位实测）。
    pub decimal_digits: usize,
    /// 身份来源描述（探针报告锚点，便于追溯）。
    pub source_note: &'static str,
}

/// 产品描述符：一个产品的全部跨平台差异点声明。
/// 新代码（捕获/备份/进程管理）按 descriptor 编排；协议细节仍由
/// 各自的实测协议模块持有（本模块不搬运协议实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductDescriptor {
    /// 稳定产品 ID（协议值；工作台上下文与前端路由使用）。
    pub product_id: &'static str,
    /// 用户可见产品名。
    pub display_name: &'static str,
    /// 客户端集成事实。
    pub client: ClientIntegration,
    /// 用户身份形态。
    pub user_id_shape: UserIdShape,
    /// 对话库相对路径（相对各自数据根）。
    pub database_relative_path: &'static str,
}

/// Trae CN（融合版，ADR-0035）描述符：静态事实与 TraeProduct::TraeCn 同源。
pub const TRAE_CN_DESCRIPTOR: ProductDescriptor = ProductDescriptor {
    product_id: "trae_cn",
    display_name: "TRAE",
    client: ClientIntegration {
        image_names: &["Trae CN"],
        executable_names: &["Trae CN.exe"],
        // Trae 按 Electron 默认 %APPDATA%/{productName} 惯例（实测一致）。
        user_data_dir_override: None,
        // Trae 无已知的本地探针 API（数据经库直读）。
        local_probe_ports: &[],
        local_probe_path: None,
    },
    user_id_shape: UserIdShape {
        decimal_digits: 16,
        source_note: "P15-0 铸造与 GetUserInfo 实测（16 位十进制 user_id）",
    },
    database_relative_path: "ModularData/ai-agent/database.db",
};

/// WorkBuddy 桌面版描述符：P17-T1/T2 探针实测事实（协议参数层 T4 按
/// CodeBuddy 协议解析后填充）。
pub const WORKBUDDY_DESCRIPTOR: ProductDescriptor = ProductDescriptor {
    product_id: "workbuddy",
    display_name: "WorkBuddy",
    client: ClientIntegration {
        image_names: &["WorkBuddy"],
        executable_names: &["WorkBuddy.exe"],
        // 探针实测：进程命令行显式 `--user-data-dir="C:\Users\{user}\.workbuddy\app"`。
        user_data_dir_override: Some(".workbuddy/app"),
        // 探针实测：LocalProbeServer 默认端口组（main-bootstrap.js）。
        local_probe_ports: &[18488, 18489, 18490],
        local_probe_path: Some("/workbuddy/probe"),
    },
    user_id_shape: UserIdShape {
        decimal_digits: 20,
        source_note: "P17-T1 隔离环境 Local Storage 实测（wb-slot-configs 内嵌 20 位平台 ID）",
    },
    // WorkBuddy 的对话数据形态（IndexedDB/LocalStorage）与 Trae 的
    // SQLCipher 布局不同，占位串仅声明字段存在；实际读取随 T4 按探针实现。
    database_relative_path: "",
};

/// 产品注册表（P17-T5 并列骨架）：全部已适配产品的 descriptor 清单。
/// 前端产品管理页/切换器的数据源；新产品适配 = 在此追加一行。
pub const ALL_PRODUCTS: &[ProductDescriptor] = &[TRAE_CN_DESCRIPTOR, WORKBUDDY_DESCRIPTOR];

/// 按 product_id 查找 descriptor（未适配产品返回 None——调用方 fail-closed）。
pub fn descriptor_by_product_id(product_id: &str) -> Option<&'static ProductDescriptor> {
    ALL_PRODUCTS
        .iter()
        .find(|descriptor| descriptor.product_id == product_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workbuddy_probe_url_matches_local_probe_server() {
        // P17-T2 实测：GET /workbuddy/probe 返回
        // {"ok":true,"app":"workbuddy-desktop","version":"5.8.3","platform":"win32"}
        assert_eq!(
            WORKBUDDY_DESCRIPTOR.client.probe_url(18488).as_deref(),
            Some("http://127.0.0.1:18488/workbuddy/probe")
        );
        // 端口组来自 main-bootstrap 的 LOCAL_PROBE_DEFAULT_PORTS。
        assert_eq!(WORKBUDDY_DESCRIPTOR.client.local_probe_ports, &[18488, 18489, 18490]);
    }

    #[test]
    fn trae_cn_descriptor_aligns_with_trae_product_facts() {
        // 与 TraeProduct::TraeCn 的静态事实同源（product_adapter.rs）。
        assert_eq!(TRAE_CN_DESCRIPTOR.product_id, "trae_cn");
        assert_eq!(TRAE_CN_DESCRIPTOR.client.image_names, &["Trae CN"]);
        assert_eq!(TRAE_CN_DESCRIPTOR.client.executable_names, &["Trae CN.exe"]);
        assert_eq!(
            TRAE_CN_DESCRIPTOR.database_relative_path,
            TraeProductShim::TRAECN_DATABASE_PATH
        );
    }

    #[test]
    fn user_id_shapes_distinguish_platforms() {
        // P17-T1 实测：WorkBuddy 20 位（2079418751248068608）；Trae 16 位。
        assert_eq!(WORKBUDDY_DESCRIPTOR.user_id_shape.decimal_digits, 20);
        assert_eq!(TRAE_CN_DESCRIPTOR.user_id_shape.decimal_digits, 16);
        assert_ne!(
            WORKBUDDY_DESCRIPTOR.product_id,
            TRAE_CN_DESCRIPTOR.product_id
        );
    }

    /// 产品注册表（P17-T5 并列骨架）：双产品在册 + 查找 fail-closed。
    #[test]
    fn product_registry_lists_and_fails_closed() {
        assert_eq!(ALL_PRODUCTS.len(), 2);
        assert_eq!(
            descriptor_by_product_id("workbuddy")
                .expect("workbuddy in registry")
                .display_name,
            "WorkBuddy"
        );
        assert_eq!(
            descriptor_by_product_id("trae_cn")
                .expect("trae_cn in registry")
                .display_name,
            "TRAE"
        );
        assert!(descriptor_by_product_id("work_cn").is_none());
        assert!(descriptor_by_product_id("").is_none());
    }

    /// 对 product_adapter 的静态事实做同源校验（避免两处漂移）。
    mod TraeProductShim {
        pub const TRAECN_DATABASE_PATH: &str = "ModularData/ai-agent/database.db";
    }
}
