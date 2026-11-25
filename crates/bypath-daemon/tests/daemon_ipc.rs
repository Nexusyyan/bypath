#![cfg(windows)]

use bypath_core::config::{ProxyConfig, ProxyKind, RuleAction, RuleConfig};
use bypath_daemon::DaemonController;
use bypath_ipc::{BackendMode, IpcClient, IpcRequest, IpcResponse, IpcServer};
use std::time::Duration;
use tokio::sync::oneshot;

#[tokio::test]
async fn daemon_controller_end_to_end_over_named_pipe() {
    let temp_dir = std::env::temp_dir().join(format!("bypath-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&temp_dir);
    let cfg_path = temp_dir.join("config.toml");
    let _ = std::fs::remove_file(&cfg_path);

    let pipe_name = format!(r"\\.\pipe\bypath-daemon-e2e-{}", std::process::id());
    let controller = DaemonController::new(cfg_path.clone(), BackendMode::Service).unwrap();
    let server = IpcServer::new(pipe_name.clone(), controller.clone());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server_task = tokio::spawn(server.run(shutdown_rx));
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = IpcClient::new(pipe_name);

    // 1. Ping & Status
    let pong = client.request(&IpcRequest::Ping).await.unwrap();
    assert!(matches!(pong, IpcResponse::Pong { .. }));

    let status = match client.request(&IpcRequest::GetStatus).await.unwrap() {
        IpcResponse::Status { status } => status,
        other => panic!("unexpected GetStatus response: {other:?}"),
    };
    assert_eq!(status.backend_mode, BackendMode::Service);
    assert!(!status.engine_running);

    // 2. Presets catalog
    let presets = match client.request(&IpcRequest::ListPresets).await.unwrap() {
        IpcResponse::Presets { presets } => presets,
        other => panic!("unexpected ListPresets response: {other:?}"),
    };
    assert!(presets.iter().any(|p| p.id == "antigravity"));
    assert!(presets.iter().any(|p| p.id == "cursor"));

    // 3. GetConfig -> mutate -> SaveConfig -> verify persisted TOML on disk
    let mut cfg = match client.request(&IpcRequest::GetConfig).await.unwrap() {
        IpcResponse::Config { snapshot } => snapshot.config,
        other => panic!("unexpected GetConfig response: {other:?}"),
    };
    cfg.proxies.insert(
        "test_http".into(),
        ProxyConfig {
            kind: ProxyKind::Http,
            address: "127.0.0.1:8080".into(),
            username: None,
            password: None,
            udp: false,
        },
    );
    cfg.rules.push(RuleConfig {
        name: "Test Rule".into(),
        enabled: true,
        process_names: vec!["curl.exe".into()],
        path_prefixes: vec![],
        path_contains: vec![],
        include_children: true,
        dst_cidrs: vec![],
        dst_ports: vec![],
        protocols: vec![],
        domains: vec![],
        action: RuleAction::Proxy,
        proxy: Some("test_http".into()),
    });

    let save_resp = client
        .request(&IpcRequest::SaveConfig {
            config: cfg,
            reload: true,
        })
        .await
        .unwrap();
    assert!(matches!(save_resp, IpcResponse::Ok { .. }));
    assert!(cfg_path.exists(), "config.toml must be written atomically");

    // 4. ListProcesses returns live Windows processes
    let procs = match client
        .request(&IpcRequest::ListProcesses { filter: None })
        .await
        .unwrap()
    {
        IpcResponse::Processes { processes } => processes,
        other => panic!("unexpected ListProcesses response: {other:?}"),
    };
    assert!(
        procs.iter().any(|p| p.pid == std::process::id()),
        "current test process must appear in ListProcesses"
    );

    // 5. Event subscription receives telemetry events cleanly
    let mut events = client.subscribe_events().await.unwrap();
    controller.telemetry().clear_closed_connections();
    let evt = tokio::time::timeout(Duration::from_secs(2), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        evt,
        bypath_core::telemetry::EngineEvent::ConnectionsCleared
    ));

    let _ = shutdown_tx.send(());
    let _ = server_task.await;
    let _ = std::fs::remove_dir_all(&temp_dir);
}
