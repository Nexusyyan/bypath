use crate::protocol::{IpcRequest, IpcResponse, DEFAULT_PIPE_NAME};
use anyhow::{anyhow, Context, Result};
use bypath_core::telemetry::EngineEvent;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, oneshot};
use tracing::{debug, info, warn};

/// Async request handler implemented by `DaemonController`.
pub trait IpcHandler: Send + Sync + 'static {
    fn handle(
        &self,
        request: IpcRequest,
    ) -> Pin<Box<dyn Future<Output = IpcResponse> + Send + '_>>;

    fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent>;
}

/// Cross-platform IPC client (Windows Named Pipe backend).
#[derive(Debug, Clone)]
pub struct IpcClient {
    endpoint: String,
}

impl Default for IpcClient {
    fn default() -> Self {
        Self::new(DEFAULT_PIPE_NAME)
    }
}

impl IpcClient {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Sends a single RPC request and awaits the matching response.
    #[cfg(windows)]
    pub async fn request(&self, req: &IpcRequest) -> Result<IpcResponse> {
        let mut pipe = connect_named_pipe(&self.endpoint).await?;
        let mut payload = serde_json::to_string(req)?;
        payload.push('\n');
        pipe.write_all(payload.as_bytes()).await?;
        pipe.flush().await?;

        let mut reader = BufReader::new(pipe);
        let mut line = String::new();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Err(anyhow!("IPC pipe closed before response"));
        }
        let resp: IpcResponse =
            serde_json::from_str(line.trim()).context("failed to decode IPC response JSON")?;
        Ok(resp)
    }

    /// Subscribes to the daemon's real-time `EngineEvent` stream over a dedicated pipe connection.
    #[cfg(windows)]
    pub async fn subscribe_events(&self) -> Result<tokio::sync::mpsc::Receiver<EngineEvent>> {
        let mut pipe = connect_named_pipe(&self.endpoint).await?;
        let mut payload = serde_json::to_string(&IpcRequest::SubscribeEvents)?;
        payload.push('\n');
        pipe.write_all(payload.as_bytes()).await?;
        pipe.flush().await?;

        let mut reader = BufReader::new(pipe);
        let mut ack_line = String::new();
        let n = reader.read_line(&mut ack_line).await?;
        if n == 0 {
            return Err(anyhow!("IPC pipe closed before subscription ACK"));
        }
        let ack: IpcResponse = serde_json::from_str(ack_line.trim())?;
        if !matches!(ack, IpcResponse::Subscribed) {
            return Err(anyhow!("unexpected subscription ACK: {ack:?}"));
        }

        let (tx, rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(evt) = serde_json::from_str::<EngineEvent>(line.trim()) {
                            if tx.send(evt).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(rx)
    }

    #[cfg(not(windows))]
    pub async fn request(&self, _req: &IpcRequest) -> Result<IpcResponse> {
        Err(anyhow!("Named Pipe IPC is currently implemented for Windows"))
    }

    #[cfg(not(windows))]
    pub async fn subscribe_events(&self) -> Result<tokio::sync::mpsc::Receiver<EngineEvent>> {
        Err(anyhow!("Named Pipe IPC is currently implemented for Windows"))
    }
}

/// Windows Named Pipe IPC server with explicit local-interactive ACLs and remote client rejection.
pub struct IpcServer {
    endpoint: String,
    handler: Arc<dyn IpcHandler>,
}

impl IpcServer {
    pub fn new(endpoint: impl Into<String>, handler: Arc<dyn IpcHandler>) -> Self {
        Self {
            endpoint: endpoint.into(),
            handler,
        }
    }

    #[cfg(windows)]
    pub async fn run(self, mut shutdown_rx: oneshot::Receiver<()>) -> Result<()> {
        let mut server = create_pipe_instance(&self.endpoint, true)?;
        info!(pipe = %self.endpoint, "IPC Named Pipe server listening");

        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    info!(pipe = %self.endpoint, "IPC Named Pipe server shutting down");
                    break;
                }
                res = server.connect() => {
                    match res {
                        Ok(()) => {
                            let connected = server;
                            // Immediately spin up the next listening pipe instance so concurrent
                            // GUI / CLI clients never encounter PIPE_BUSY gaps.
                            server = match create_pipe_instance(&self.endpoint, false) {
                                Ok(next) => next,
                                Err(err) => {
                                    warn!(?err, "failed to create next Named Pipe instance");
                                    break;
                                }
                            };
                            let handler = Arc::clone(&self.handler);
                            tokio::spawn(async move {
                                if let Err(err) = handle_pipe_connection(connected, handler).await {
                                    debug!(?err, "IPC client session ended");
                                }
                            });
                        }
                        Err(err) => {
                            warn!(?err, "Named Pipe connect error");
                        }
                    }
                }
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    pub async fn run(self, mut shutdown_rx: oneshot::Receiver<()>) -> Result<()> {
        let _ = (&self.endpoint, &self.handler);
        let _ = (&mut shutdown_rx).await;
        Ok(())
    }
}

#[cfg(windows)]
async fn handle_pipe_connection(
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    handler: Arc<dyn IpcHandler>,
) -> Result<()> {
    let (read_half, mut write_half) = tokio::io::split(pipe);
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    let n = reader.read_line(&mut line).await?;
    if n == 0 {
        return Ok(());
    }

    let req: IpcRequest = match serde_json::from_str(line.trim()) {
        Ok(r) => r,
        Err(err) => {
            let mut err_json = serde_json::to_string(&IpcResponse::Error {
                message: format!("invalid request JSON: {err}"),
            })?;
            err_json.push('\n');
            write_half.write_all(err_json.as_bytes()).await?;
            write_half.flush().await?;
            return Ok(());
        }
    };

    if matches!(req, IpcRequest::SubscribeEvents) {
        let mut ack = serde_json::to_string(&IpcResponse::Subscribed)?;
        ack.push('\n');
        write_half.write_all(ack.as_bytes()).await?;
        write_half.flush().await?;

        let mut rx = handler.subscribe_events();
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let mut out = serde_json::to_string(&evt)?;
                    out.push('\n');
                    if write_half.write_all(out.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    debug!(skipped, "IPC subscriber lagged behind broadcast");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        return Ok(());
    }

    let response = handler.handle(req).await;
    let mut out = serde_json::to_string(&response)?;
    out.push('\n');
    write_half.write_all(out.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

#[cfg(windows)]
async fn connect_named_pipe(
    endpoint: &str,
) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;

    for attempt in 0..8 {
        match ClientOptions::new().open(endpoint) {
            Ok(client) => return Ok(client),
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempt < 7 => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(err) => {
                return Err(anyhow!(
                    "failed to connect to IPC pipe '{endpoint}': {err}"
                ));
            }
        }
    }
    Err(anyhow!("IPC pipe '{endpoint}' remained busy"))
}

#[cfg(windows)]
fn create_pipe_instance(
    endpoint: &str,
    first_instance: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use std::ffi::c_void;
    use tokio::net::windows::named_pipe::ServerOptions;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

    // Allow LocalSystem (SY) and Built-in Administrators (BA) full control, and local
    // Interactive Users (IU) read/write so a non-elevated desktop GUI can control an elevated
    // Windows Service while rejecting remote network clients.
    const SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;IU)";

    struct OwnedSd(PSECURITY_DESCRIPTOR);
    impl Drop for OwnedSd {
        fn drop(&mut self) {
            if !self.0 .0.is_null() {
                unsafe {
                    let _ = LocalFree(HLOCAL(self.0 .0));
                }
            }
        }
    }

    let mut wide: Vec<u16> = SDDL.encode_utf16().collect();
    wide.push(0);

    let mut sd = PSECURITY_DESCRIPTOR::default();
    let sd_ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut sd,
            None,
        )
    };

    let mut opts = ServerOptions::new();
    opts.first_pipe_instance(first_instance)
        .reject_remote_clients(true);

    if sd_ok.is_ok() {
        let _guard = OwnedSd(sd);
        let mut sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd.0,
            bInheritHandle: false.into(),
        };
        let server = unsafe {
            opts.create_with_security_attributes_raw(
                endpoint,
                (&mut sa as *mut SECURITY_ATTRIBUTES).cast::<c_void>(),
            )
        }
        .with_context(|| format!("failed to create Named Pipe '{endpoint}' with custom ACL"))?;
        Ok(server)
    } else {
        opts.create(endpoint)
            .with_context(|| format!("failed to create Named Pipe '{endpoint}'"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use bypath_core::telemetry::TelemetryHub;

    struct DummyHandler {
        hub: Arc<TelemetryHub>,
    }

    impl IpcHandler for DummyHandler {
        fn handle(
            &self,
            request: IpcRequest,
        ) -> Pin<Box<dyn Future<Output = IpcResponse> + Send + '_>> {
            Box::pin(async move {
                match request {
                    IpcRequest::Ping => IpcResponse::Pong {
                        version: "test-0.2.0".into(),
                    },
                    _ => IpcResponse::Ok {
                        message: "handled".into(),
                    },
                }
            })
        }

        fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
            self.hub.subscribe()
        }
    }

    #[tokio::test]
    async fn named_pipe_rpc_and_event_streaming_roundtrip() {
        let pipe_name = format!(r"\\.\pipe\bypath-test-{}", std::process::id());
        let hub = TelemetryHub::new();
        let handler = Arc::new(DummyHandler {
            hub: Arc::clone(&hub),
        });

        let server = IpcServer::new(pipe_name.clone(), handler);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server_task = tokio::spawn(server.run(shutdown_rx));

        // Give server loop a brief moment to create the pipe instance.
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;

        let client = IpcClient::new(pipe_name);
        let pong = client.request(&IpcRequest::Ping).await.unwrap();
        assert!(matches!(
            pong,
            IpcResponse::Pong { ref version } if version == "test-0.2.0"
        ));

        let mut events = client.subscribe_events().await.unwrap();
        hub.notify_config_reloaded(3, 1);

        let evt = tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            evt,
            EngineEvent::ConfigReloaded {
                rules_count: 3,
                proxies_count: 1
            }
        ));

        let _ = shutdown_tx.send(());
        let _ = server_task.await;
    }
}
