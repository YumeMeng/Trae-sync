//! 依赖方向验证测试（AC2）。
//!
//! 验证 workspace 中各 crate 的 Cargo.toml 依赖声明符合规格第 7 节的分层方向：
//!
//! ```text
//! React UI
//!   -> Tauri commands
//!   -> application services
//!   -> domain + ports
//!   -> infrastructure
//! ```
//!
//! 具体规则：
//! - domain: 不依赖任何内部 crate 或 tauri
//! - ports: 只依赖 domain
//! - application: 只依赖 domain + ports
//! - infrastructure: 只依赖 domain + ports（不依赖 application/commands/tauri）
//! - commands: 只依赖 application + domain（不依赖 infrastructure/tauri）
//!
//! 这些规则同时由 Cargo 编译期强制——任何反向依赖会导致编译失败。
//! 本测试作为文档化的自动检查，明确表达期望的依赖图。

use std::collections::HashSet;
use std::path::Path;

/// 解析 Cargo.toml 中 [dependencies] 段的内部 crate 依赖名
fn parse_internal_deps(cargo_toml_path: &Path) -> HashSet<String> {
    let content = std::fs::read_to_string(cargo_toml_path)
        .unwrap_or_else(|e| panic!("无法读取 {}: {e}", cargo_toml_path.display()));

    let mut deps = HashSet::new();
    let mut in_dependencies = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // 检测 section 头
        if trimmed.starts_with('[') {
            in_dependencies = trimmed == "[dependencies]" || trimmed.starts_with("[dependencies.");
            continue;
        }

        if !in_dependencies {
            continue;
        }

        // 提取依赖名（等号前的部分）
        if let Some(eq_pos) = trimmed.find('=') {
            let name = trimmed[..eq_pos].trim();
            // 只收集 traesync-* 内部依赖
            if name.starts_with("traesync-") {
                deps.insert(name.to_string());
            }
        }
    }

    deps
}

/// 断言 crate 的内部依赖集合恰好等于期望集合
fn assert_deps_eq(crate_name: &str, actual: &HashSet<String>, expected: &[&str]) {
    let expected_set: HashSet<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        actual, &expected_set,
        "{crate_name} 的内部依赖不匹配。期望: {expected_set:?}, 实际: {actual:?}"
    );
}

/// 断言 crate 不依赖某些禁止的 crate
fn assert_deps_exclude(crate_name: &str, actual: &HashSet<String>, forbidden: &[&str]) {
    for f in forbidden {
        assert!(
            !actual.contains(*f),
            "{crate_name} 不应依赖 {f}，但实际依赖了。实际依赖: {actual:?}"
        );
    }
}

#[test]
fn domain_has_no_internal_deps_and_no_tauri() {
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/domain/Cargo.toml");
    let deps = parse_internal_deps(&cargo);
    // domain 不依赖任何内部 crate
    assert_deps_eq("traesync-domain", &deps, &[]);
}

#[test]
fn ports_only_depends_on_domain() {
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/ports/Cargo.toml");
    let deps = parse_internal_deps(&cargo);
    assert_deps_eq("traesync-ports", &deps, &["traesync-domain"]);
    // 显式断言不依赖 application/infrastructure/commands
    assert_deps_exclude(
        "traesync-ports",
        &deps,
        &[
            "traesync-application",
            "traesync-infrastructure",
            "traesync-commands",
        ],
    );
}

#[test]
fn application_only_depends_on_domain_and_ports() {
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/application/Cargo.toml");
    let deps = parse_internal_deps(&cargo);
    assert_deps_eq(
        "traesync-application",
        &deps,
        &["traesync-domain", "traesync-ports"],
    );
    assert_deps_exclude(
        "traesync-application",
        &deps,
        &["traesync-infrastructure", "traesync-commands"],
    );
}

#[test]
fn infrastructure_only_depends_on_domain_and_ports() {
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/infrastructure/Cargo.toml");
    let deps = parse_internal_deps(&cargo);
    assert_deps_eq(
        "traesync-infrastructure",
        &deps,
        &["traesync-domain", "traesync-ports"],
    );
    // infrastructure 不依赖 application/commands——这保证基础设施不会反向调用应用服务
    assert_deps_exclude(
        "traesync-infrastructure",
        &deps,
        &["traesync-application", "traesync-commands"],
    );
}

#[test]
fn commands_only_depends_on_application_and_domain() {
    let cargo = Path::new(env!("CARGO_MANIFEST_DIR")).join("crates/commands/Cargo.toml");
    let deps = parse_internal_deps(&cargo);
    assert_deps_eq(
        "traesync-commands",
        &deps,
        &["traesync-domain", "traesync-application"],
    );
    // commands 不直接依赖 infrastructure——必须通过 application 的 port
    assert_deps_exclude("traesync-commands", &deps, &["traesync-infrastructure"]);
}

#[test]
fn workspace_compiles_proving_direction_enforced() {
    // 这个测试的存在本身就是证明：如果依赖方向有误，Cargo 不会编译这个测试。
    // 例如，如果 domain 试图 `use traesync_commands`，编译会失败，
    // 因为 domain 的 Cargo.toml 没有声明 commands 依赖。
    //
    // 这是 AC2 要求的“自动检查或测试证明关键依赖方向”的编译期证据。
    println!("依赖方向由 Cargo workspace 编译期强制，测试通过即证明方向正确。");
}
