//! T13 恢复应用服务：先完成包与目录库验证，再编排密钥包装。

use std::fmt;

use traesync_ports::{
    KeyWrapperError, KeyWrapperPort, KeyWrapperRequest, RecoveryImportError, RecoveryImportRequest,
    RecoveryPackageImportPort,
};

/// 恢复成功后对上层可见的非敏感结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryImportResult {
    pub catalog_id: String,
    pub key_generation: u32,
    pub schema_version: u32,
    pub key_wrapper_version: u32,
}

/// 恢复编排错误；密钥材料不会进入错误值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryImportServiceError {
    Import(RecoveryImportError),
    Wrapper(KeyWrapperError),
    VerifiedMaterialMismatch,
    WrapperVersionMismatch,
}

impl fmt::Display for RecoveryImportServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Import(error) => error.fmt(formatter),
            Self::Wrapper(error) => error.fmt(formatter),
            Self::VerifiedMaterialMismatch => formatter.write_str("已验证恢复材料与请求不匹配"),
            Self::WrapperVersionMismatch => formatter.write_str("密钥包装器版本与恢复包不匹配"),
        }
    }
}

/// 恢复应用服务只依赖两个端口，便于在不触碰文件系统的情况下验证调用顺序。
pub struct RecoveryImportService<'a> {
    importer: &'a dyn RecoveryPackageImportPort,
    key_wrapper: &'a dyn KeyWrapperPort,
}

impl<'a> RecoveryImportService<'a> {
    pub fn new(
        importer: &'a dyn RecoveryPackageImportPort,
        key_wrapper: &'a dyn KeyWrapperPort,
    ) -> Self {
        Self {
            importer,
            key_wrapper,
        }
    }

    /// 只有导入端口完成认证、只读打开和身份验证后，才允许调用包装器。
    pub fn import(
        &self,
        request: &RecoveryImportRequest<'_>,
    ) -> Result<RecoveryImportResult, RecoveryImportServiceError> {
        let material = self
            .importer
            .import_and_verify(request)
            .map_err(RecoveryImportServiceError::Import)?;

        if material.catalog_id() != request.expected_catalog_id
            || material.key_generation() != request.expected_key_generation
            || material.schema_version() != request.expected_schema_version
        {
            return Err(RecoveryImportServiceError::VerifiedMaterialMismatch);
        }

        // 先查询支持版本，避免版本不匹配时已经写入新的 DPAPI 包装文件。
        let supported_wrapper_version = self.key_wrapper.supported_wrapper_version();
        if material.key_wrapper_version() != supported_wrapper_version {
            return Err(RecoveryImportServiceError::WrapperVersionMismatch);
        }

        let wrapper_request = KeyWrapperRequest::new(
            material.catalog_id(),
            material.key_generation(),
            material.catalog_key_hex(),
        );
        let receipt = self
            .key_wrapper
            .wrap_catalog_key(&wrapper_request)
            .map_err(RecoveryImportServiceError::Wrapper)?;

        if receipt.wrapper_version != material.key_wrapper_version()
            || receipt.wrapper_version != supported_wrapper_version
        {
            return Err(RecoveryImportServiceError::WrapperVersionMismatch);
        }

        Ok(RecoveryImportResult {
            catalog_id: material.catalog_id().to_string(),
            key_generation: material.key_generation(),
            schema_version: material.schema_version(),
            key_wrapper_version: receipt.wrapper_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use traesync_ports::{
        KeyWrapperPort, KeyWrapperReceipt, RecoveryImportRequest, RecoveryPackageImportPort,
        VerifiedRecoveryMaterial,
    };

    use super::{RecoveryImportService, RecoveryImportServiceError};

    struct FixedImporter {
        result: Result<VerifiedRecoveryMaterial, traesync_ports::RecoveryImportError>,
    }

    impl RecoveryPackageImportPort for FixedImporter {
        fn import_and_verify(
            &self,
            _request: &RecoveryImportRequest<'_>,
        ) -> Result<VerifiedRecoveryMaterial, traesync_ports::RecoveryImportError> {
            self.result.clone()
        }
    }

    struct CountingWrapper {
        calls: AtomicUsize,
        version: u32,
    }

    impl KeyWrapperPort for CountingWrapper {
        fn supported_wrapper_version(&self) -> u32 {
            self.version
        }

        fn wrap_catalog_key(
            &self,
            _request: &traesync_ports::KeyWrapperRequest<'_>,
        ) -> Result<KeyWrapperReceipt, traesync_ports::KeyWrapperError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(KeyWrapperReceipt {
                wrapper_version: self.version,
            })
        }
    }

    fn request<'a>(package: &'a Path, catalog: &'a Path) -> RecoveryImportRequest<'a> {
        RecoveryImportRequest {
            package_path: package,
            catalog_path: catalog,
            passphrase: "password",
            expected_catalog_id: "catalog-a",
            expected_key_generation: 7,
            expected_schema_version: 1,
        }
    }

    fn material() -> VerifiedRecoveryMaterial {
        VerifiedRecoveryMaterial::new("catalog-a".to_string(), 7, 1, "ab".repeat(32), 1)
    }

    #[test]
    fn importer_failure_never_calls_key_wrapper() {
        let importer = FixedImporter {
            result: Err(traesync_ports::RecoveryImportError::WrongPassphrase),
        };
        let wrapper = CountingWrapper {
            calls: AtomicUsize::new(0),
            version: 1,
        };
        let service = RecoveryImportService::new(&importer, &wrapper);

        let error = service
            .import(&request(Path::new("package"), Path::new("catalog")))
            .unwrap_err();

        assert_eq!(
            error,
            RecoveryImportServiceError::Import(
                traesync_ports::RecoveryImportError::WrongPassphrase
            )
        );
        assert_eq!(wrapper.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn verified_material_is_wrapped_once() {
        let importer = FixedImporter {
            result: Ok(material()),
        };
        let wrapper = CountingWrapper {
            calls: AtomicUsize::new(0),
            version: 1,
        };
        let service = RecoveryImportService::new(&importer, &wrapper);

        let result = service
            .import(&request(Path::new("package"), Path::new("catalog")))
            .unwrap();

        assert_eq!(result.catalog_id, "catalog-a");
        assert_eq!(result.key_generation, 7);
        assert_eq!(result.schema_version, 1);
        assert_eq!(result.key_wrapper_version, 1);
        assert_eq!(wrapper.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wrapper_version_mismatch_is_rejected_before_wrapper_write() {
        let importer = FixedImporter {
            result: Ok(material()),
        };
        let wrapper = CountingWrapper {
            calls: AtomicUsize::new(0),
            version: 2,
        };
        let service = RecoveryImportService::new(&importer, &wrapper);

        let error = service
            .import(&request(Path::new("package"), Path::new("catalog")))
            .unwrap_err();

        assert_eq!(error, RecoveryImportServiceError::WrapperVersionMismatch);
        assert_eq!(wrapper.calls.load(Ordering::SeqCst), 0);
    }
}
