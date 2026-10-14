//! Async client for the lianli-daemon Unix socket IPC protocol.
//!
//! Newline-delimited JSON, one response per request, no request id — so
//! every call opens a fresh connection, sends one line, reads one line.

use anyhow::{anyhow, bail, Context, Result};
use async_io::Timer;
use async_net::unix::UnixStream;
use futures_lite::future::or;
use futures_lite::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use lianli_shared::daemon::DaemonInfo;
use lianli_shared::ipc::{IpcRequest, IpcResponse};
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::time::Duration;

/// `SetRgbFrames` only acks once the daemon finishes the whole chunked RF
/// transfer, which can legitimately take a while for a long animation — so
/// this has to stay generous. It only exists to bound a genuinely wedged
/// daemon (stuck socket, deadlock), not to catch normal slowness; a call
/// with no timeout at all hangs its caller's `.await` forever, leaving a
/// page stuck on "loading" with no way to tell a hang from real latency.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct IpcClient {
    socket_path: PathBuf,
}

impl IpcClient {
    pub fn new() -> Self {
        let runtime_dir =
            std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
        Self {
            socket_path: PathBuf::from(format!("{runtime_dir}/lianli-daemon.sock")),
        }
    }

    pub async fn call<T: DeserializeOwned>(&self, request: IpcRequest) -> Result<T> {
        let value = self.call_raw(request).await?;
        serde_json::from_value(value).context("unexpected response shape from daemon")
    }

    /// Same as `call`, for requests with a `null` response payload.
    pub async fn call_unit(&self, request: IpcRequest) -> Result<()> {
        self.call_raw(request).await?;
        Ok(())
    }

    async fn call_raw(&self, request: IpcRequest) -> Result<serde_json::Value> {
        let request = self.guard_if_needed(request).await?;
        self.send(request).await
    }

    async fn send(&self, request: IpcRequest) -> Result<serde_json::Value> {
        or(self.call_raw_inner(request), Self::timeout()).await
    }

    /// The daemon now rejects any mutating request that isn't wrapped in
    /// `IpcRequest::Guarded` with a fresh `WriteGuard` (see
    /// `lianli_shared::daemon::DaemonInfo::write_guard`) — a staleness check
    /// meant to stop a client talking to a daemon instance it hasn't just
    /// queried. Goes through `send` directly (not `call_raw`) since
    /// `GetDaemonInfo` is read-only and needs no guard of its own.
    async fn guard_if_needed(&self, request: IpcRequest) -> Result<IpcRequest> {
        if request.is_read_only() {
            return Ok(request);
        }
        let value = self.send(IpcRequest::GetDaemonInfo).await?;
        let info: DaemonInfo =
            serde_json::from_value(value).context("unexpected response shape from daemon")?;
        let guard = info.write_guard(&info.version).map_err(|e| anyhow!(e))?;
        Ok(IpcRequest::Guarded { guard, request: Box::new(request) })
    }

    async fn timeout() -> Result<serde_json::Value> {
        Timer::after(CALL_TIMEOUT).await;
        bail!("daemon did not respond within {}s", CALL_TIMEOUT.as_secs())
    }

    async fn call_raw_inner(&self, request: IpcRequest) -> Result<serde_json::Value> {
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .with_context(|| {
                format!(
                    "failed to connect to lianli-daemon at {}; is the service running?",
                    self.socket_path.display()
                )
            })?;

        let mut line = serde_json::to_string(&request)?;
        line.push('\n');

        let (read_half, mut write_half) = (stream.clone(), stream);
        write_half.write_all(line.as_bytes()).await?;
        write_half.flush().await?;

        let mut reader = BufReader::new(read_half);
        let mut response_line = String::new();
        reader.read_line(&mut response_line).await?;

        if response_line.trim().is_empty() {
            bail!("daemon closed connection without a response");
        }

        let response: IpcResponse = serde_json::from_str(&response_line)
            .context("failed to parse daemon response as JSON")?;

        match response {
            IpcResponse::Ok { data } => Ok(data),
            IpcResponse::Error { message, .. } => bail!("daemon error: {message}"),
        }
    }
}
