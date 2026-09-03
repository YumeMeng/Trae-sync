//! T13 恢复编排端口：把恢复包验证与密钥包装保持在两个窄边界内。
//!
//! `KeyWrapperPort` 不接收路径、密码或包对象，只接收已经通过认证和目录库
//! 只读验证的目录库密钥材料。这样应用层可以用 fake 验证调用顺序，而不会
//! 把包装实现、DPAPI 或文件系统细节带入应用服务。

use std::fmt;
use std::path::Path;

/// 应用层请求基础设施导入并验证恢复包。
pub struct RecoveryImportRequest<'a> {
    pub package_path: &'a Path,
    pub catalog_path: &'a Path,
    pub passphrase: &'a str,
    pub expected_catalog_id: &'a str,
    pub expected_key_generation: u32,
    pub expected_schema_version: u32,
}

impl fmt::Debug for RecoveryImportRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RecoveryImportRequest")
            .field("package_path", &self.package_path)
            .field("catalog_path", &self.catalog_path)
            .field("passphrase", &"<redacted>")
            .field("expected_catalog_id", &self.expected_catalog_id)
            .field("expected_key_generation", &self.expected_key_generation)
            .field("expected_schema_version", &self.expected_schema_version)
            .finish()
    }
}

/// 导入适配器返回的已验证密钥材料。
#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedRecoveryMaterial {
    catalog_id: String,
    key_generation: u32,
    schema_version: u32,
    catalog_key_hex: String,
    key_wrapper_version: u32,
}

impl VerifiedRecoveryMaterial {
    /// 由 infrastructure 创建已经通过只读验证的材料。
    pub fn new(
        catalog_id: String,
        key_generation: u32,
        schema_version: u32,
        catalog_key_hex: String,
        key_wrapper_version: u32,
    ) -> Self {
        Self {
            catalog_id,
            key_generation,
            schema_version,
            catalog_key_hex,
            key_wrapper_version,
        }
    }

    pub fn catalog_id(&self) -> &str {
        &self.catalog_id
    }

    pub fn key_generation(&self) -> u32 {
        self.key_generation
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn catalog_key_hex(&self) -> &str {
        &self.catalog_key_hex
    }

    pub fn key_wrapper_version(&self) -> u32 {
        self.key_wrapper_version
    }
}

impl fmt::Debug for VerifiedRecoveryMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedRecoveryMaterial")
            .field("catalog_id", &self.catalog_id)
            .field("key_generation", &self.key_generation)
            .field("schema_version", &self.schema_version)
            .field("catalog_key_hex", &"<redacted>")
            .field("key_wrapper_version", &self.key_wrapper_version)
            .finish()
    }
}

/// 恢复包验证失败的稳定端口错误；不把底层 SQLCipher/文件错误泄漏到上层。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryImportError {
    EmptyPassphrase,
    WrongPassphrase,
    InvalidPackage,
    IdentityMismatch,
    InsufficientSpace,
    CatalogVerificationFailed,
    Io,
    UnsupportedMetadata,
}

impl fmt::Display for RecoveryImportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EmptyPassphrase => "恢复密码不能为空",
            Self::WrongPassphrase => "恢复密码错误或认证失败",
            Self::InvalidPackage => "恢复包格式无效",
            Self::IdentityMismatch => "恢复包与目录库身份不匹配",
            Self::InsufficientSpace => "升级所需可用空间不足",
            Self::CatalogVerificationFailed => "目录库只读验证失败",
            Self::Io => "恢复包读取失败",
            Self::UnsupportedMetadata => "恢复包版本矩阵不兼容",
        };
        formatter.write_str(message)
    }
}

/// 恢复包导入与目录库只读验证端口。
pub trait RecoveryPackageImportPort: Send + Sync {
    fn import_and_verify(
        &self,
        request: &RecoveryImportRequest<'_>,
    ) -> Result<VerifiedRecoveryMaterial, RecoveryImportError>;
}

#[cfg(test)]
mod tests {
    use super::RecoveryImportError;

    #[test]
    fn insufficient_space_has_stable_display() {
        assert_eq!(
            RecoveryImportError::InsufficientSpace.to_string(),
            "升级所需可用空间不足"
        );
    }
}

/// 传给密钥包装器的最小请求，不包含包路径、恢复密码或目录库路径。
pub struct KeyWrapperRequest<'a> {
    catalog_id: &'a str,
    key_generation: u32,
    catalog_key_hex: &'a str,
}

impl<'a> KeyWrapperRequest<'a> {
    pub fn new(catalog_id: &'a str, key_generation: u32, catalog_key_hex: &'a str) -> Self {
        Self {
            catalog_id,
            key_generation,
            catalog_key_hex,
        }
    }

    pub fn catalog_id(&self) -> &str {
        self.catalog_id
    }

    pub fn key_generation(&self) -> u32 {
        self.key_generation
    }

    pub fn catalog_key_hex(&self) -> &str {
        self.catalog_key_hex
    }
}

impl fmt::Debug for KeyWrapperRequest<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KeyWrapperRequest")
            .field("catalog_id", &self.catalog_id)
            .field("key_generation", &self.key_generation)
            .field("catalog_key_hex", &"<redacted>")
            .finish()
    }
}

/// 包装器返回的版本信息，用于校验版本矩阵没有在编排过程中漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyWrapperReceipt {
    pub wrapper_version: u32,
}

/// 密钥包装失败的稳定端口错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyWrapperError {
    Unavailable,
    Failed,
}

impl fmt::Display for KeyWrapperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Unavailable => "密钥包装器不可用",
            Self::Failed => "密钥包装失败",
        };
        formatter.write_str(message)
    }
}

/// 窄密钥包装端口：只有验证成功后的应用编排才可以调用。
pub trait KeyWrapperPort: Send + Sync {
    /// 返回当前包装器支持的版本；应用层会在任何写入前与恢复包版本比较。
    fn supported_wrapper_version(&self) -> u32;

    fn wrap_catalog_key(
        &self,
        request: &KeyWrapperRequest<'_>,
    ) -> Result<KeyWrapperReceipt, KeyWrapperError>;
}
