//! 目录库当前指针解析。
//!
//! 这里只做文件布局和符号链接边界检查，不打开 SQLite。这样存储根迁移和轻量
//! fixture 测试不必依赖 SQLCipher/OpenSSL 编译；真正目录库读写仍由 `catalog` 模块负责。

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 当前目录库指针解析失败原因；所有失败都保持关闭，不创建替代数据库。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogPathError {
    Missing,
    Invalid,
    Io,
    InitializationFailed,
    CatalogWriteProtocolUpgradeRequired,
    LeaseContextMismatch,
}

impl CatalogPathError {
    /// 返回可跨 IPC 和测试稳定匹配的非敏感错误码。
    pub const fn code(self) -> &'static str {
        match self {
            Self::Missing => "catalog_missing",
            Self::Invalid => "catalog_invalid",
            Self::Io => "catalog_io_error",
            Self::InitializationFailed => "catalog_initialization_failed",
            Self::CatalogWriteProtocolUpgradeRequired => "catalog_write_protocol_upgrade_required",
            Self::LeaseContextMismatch => "catalog_lease_context_mismatch",
        }
    }
}

impl std::fmt::Display for CatalogPathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::Missing => "当前目录库不存在",
            Self::Invalid => "当前目录库指针或代次无效",
            Self::Io => "目录库布局读写失败",
            Self::InitializationFailed => "目录库初始化失败",
            Self::CatalogWriteProtocolUpgradeRequired => "当前目录库采用了此版本不支持的写入协议",
            Self::LeaseContextMismatch => "目录库租约上下文不匹配",
        };
        f.write_str(message)
    }
}

impl std::error::Error for CatalogPathError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CatalogCurrentPointer {
    pub(crate) generation_id: String,
}

/// 解析 `<storage_root>/catalog/current.json`，绝不通过 `Connection::open` 创建空库。
pub fn resolve_current_catalog_path(storage_root: &Path) -> Result<PathBuf, CatalogPathError> {
    let catalog_root = storage_root.join("catalog");
    let catalog_metadata = fs::symlink_metadata(&catalog_root).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CatalogPathError::Missing
        } else {
            CatalogPathError::Io
        }
    })?;
    if catalog_metadata.file_type().is_symlink() || !catalog_metadata.is_dir() {
        return Err(CatalogPathError::Invalid);
    }

    let pointer_path = catalog_root.join("current.json");
    let pointer_metadata = fs::symlink_metadata(&pointer_path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CatalogPathError::Missing
        } else {
            CatalogPathError::Io
        }
    })?;
    if pointer_metadata.file_type().is_symlink() || !pointer_metadata.is_file() {
        return Err(CatalogPathError::Invalid);
    }
    let pointer: CatalogCurrentPointer =
        serde_json::from_reader(File::open(&pointer_path).map_err(|_| CatalogPathError::Io)?)
            .map_err(|_| CatalogPathError::Invalid)?;
    validate_generation_id(&pointer.generation_id)?;

    let generations_root = catalog_root.join("generations");
    let generations_metadata =
        fs::symlink_metadata(&generations_root).map_err(|_| CatalogPathError::Invalid)?;
    if generations_metadata.file_type().is_symlink() || !generations_metadata.is_dir() {
        return Err(CatalogPathError::Invalid);
    }
    let generation_dir = generations_root.join(&pointer.generation_id);
    let generation_metadata =
        fs::symlink_metadata(&generation_dir).map_err(|_| CatalogPathError::Invalid)?;
    if generation_metadata.file_type().is_symlink() || !generation_metadata.is_dir() {
        return Err(CatalogPathError::Invalid);
    }
    let catalog_path = generation_dir.join("catalog.db");
    let catalog_metadata =
        fs::symlink_metadata(&catalog_path).map_err(|_| CatalogPathError::Invalid)?;
    if catalog_metadata.file_type().is_symlink() || !catalog_metadata.is_file() {
        return Err(CatalogPathError::Invalid);
    }
    Ok(catalog_path)
}

/// 返回已完成完整布局校验的当前目录库代次标识。
pub fn resolve_current_catalog_generation_id(
    storage_root: &Path,
) -> Result<String, CatalogPathError> {
    let catalog_path = resolve_current_catalog_path(storage_root)?;
    catalog_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or(CatalogPathError::Invalid)
}

pub(crate) fn validate_generation_id(generation_id: &str) -> Result<(), CatalogPathError> {
    if generation_id.is_empty()
        || generation_id == "."
        || generation_id == ".."
        || generation_id
            .chars()
            .any(|character| matches!(character, '/' | '\\' | ':'))
    {
        return Err(CatalogPathError::Invalid);
    }
    Ok(())
}
