use std::{fs, path::Path};

#[test]
fn cargo_manifest_ships_exactly_one_xiao_binary() {
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let package = manifest
        .get("package")
        .and_then(toml::Value::as_table)
        .unwrap();
    assert_eq!(
        package.get("name").and_then(toml::Value::as_str),
        Some("xiao")
    );
    assert_eq!(
        package.get("version").and_then(toml::Value::as_str),
        Some("0.3.0")
    );

    let binaries = manifest.get("bin").and_then(toml::Value::as_array).unwrap();
    assert_eq!(binaries.len(), 1, "v0.3 ships one native executable");
    let binary = binaries[0].as_table().unwrap();
    assert_eq!(
        binary.get("name").and_then(toml::Value::as_str),
        Some("xiao")
    );
    assert_eq!(
        binary.get("path").and_then(toml::Value::as_str),
        Some("src/main.rs")
    );
}

#[test]
fn unified_entrypoint_modules_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in ["src/cli/mod.rs", "src/runtime/host.rs"] {
        assert!(root.join(path).is_file(), "missing {path}");
    }
    assert!(!root.join("src/bin_cli.rs").exists());
}

#[test]
fn android_launchers_use_xiao_daemon_without_a_second_binary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [
        "module/common.sh",
        "module/watchdog.sh",
        "module/customize.sh",
        "module/action.sh",
        "module/uninstall.sh",
        "packaging/build-module.sh",
        "scripts/device-custom-e2e.sh",
        ".github/workflows/ci.yml",
    ] {
        let source = fs::read_to_string(root.join(path)).unwrap();
        assert!(!source.contains("bin/xiaod"), "{path} still ships xiaod");
        assert!(!source.contains("--bin xiaod"), "{path} still builds xiaod");
        assert!(
            !source.contains("XIAOD_BINARY"),
            "{path} still references a second binary"
        );
    }
    let watchdog = fs::read_to_string(root.join("module/watchdog.sh")).unwrap();
    assert!(watchdog.contains("\"$XIAO_BINARY\" daemon"));
    let packaging = fs::read_to_string(root.join("packaging/build-module.sh")).unwrap();
    assert!(packaging.contains("exactly one regular native executable"));
}
