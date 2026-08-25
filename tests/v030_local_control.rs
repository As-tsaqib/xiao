use std::{fs, path::Path};

use xiao::{
    config::AppConfig,
    security::secrets::SecretStore,
    standalone::{
        initialize, provision_client_config, CliPaths, ClientConfig, RuntimeLayout, RuntimeLock,
    },
};

fn paths(root: &Path) -> CliPaths {
    CliPaths {
        config: root.join("config/config.toml"),
        client_config: root.join("config/client.toml"),
        default_data_dir: root.join("data"),
    }
}

#[test]
fn runtime_layout_uses_private_run_socket_and_exclusive_lock() {
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path());
    let config = AppConfig::standalone(paths.default_data_dir.clone());
    let layout = RuntimeLayout::from_config(&paths, &config);

    assert_eq!(layout.run_dir, layout.data_dir.join("run"));
    assert_eq!(layout.control_socket, layout.run_dir.join("control.sock"));
    assert_eq!(layout.runtime_lock, layout.run_dir.join("runtime.lock"));

    let first = RuntimeLock::acquire(&layout).unwrap();
    assert!(RuntimeLock::acquire(&layout).is_err());
    drop(first);
    RuntimeLock::acquire(&layout).unwrap();
}

#[test]
fn generated_client_config_targets_the_unix_control_socket() {
    let directory = tempfile::tempdir().unwrap();
    let paths = paths(directory.path());
    let init = initialize(&paths).unwrap();
    SecretStore::new(init.runtime.secrets_dir.clone())
        .put("ipc-client-token", "private-token")
        .unwrap();

    assert!(provision_client_config(&paths, &init.config, &init.runtime).unwrap());
    let client = ClientConfig::load(&paths.client_config).unwrap();
    assert_eq!(
        client.control_socket,
        Some(init.runtime.control_socket.clone())
    );
    assert_eq!(client.token, "private-token");
    assert!(!fs::read_to_string(&paths.client_config)
        .unwrap()
        .contains("principal = \"owner:"));
}

#[test]
fn runtime_host_holds_lock_and_ipc_uses_private_unix_listener() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let host = fs::read_to_string(root.join("src/runtime/host.rs")).unwrap();
    let ipc = fs::read_to_string(root.join("src/ipc/mod.rs")).unwrap();

    assert!(host.contains("RuntimeLock::acquire"));
    assert!(host.contains("runtime_lock"));
    assert!(ipc.contains("UnixListener::bind"));
    assert!(ipc.contains("DefaultBodyLimit::max"));
    assert!(!ipc.contains("TcpListener::bind(addr)"));
}
