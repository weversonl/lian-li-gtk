//! Async client for the lianli-daemon Unix socket IPC protocol.
//!
//! Protocol: newline-delimited JSON. Each request line gets exactly one
//! response line back, in order — the daemon reads requests off a connection
//! sequentially and writes a response after each one (see
//! `lianli-daemon/src/ipc_server.rs::handle_connection`). There is no request
//! id to correlate out-of-order replies, so every call here opens a fresh
//! connection, sends one line, reads one line, and closes.

use anyhow::{bail, Context, Result};
use async_net::unix::UnixStream;
use futures_lite::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use lianli_shared::ipc::{IpcRequest, IpcResponse};
use serde::de::DeserializeOwned;
use std::path::PathBuf;

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

    /// Send a request and deserialize the `data` payload of an `ok` response
    /// into `T`. Errors if the daemon is unreachable or returns `status: error`.
    pub async fn call<T: DeserializeOwned>(&self, request: IpcRequest) -> Result<T> {
        let value = self.call_raw(request).await?;
        serde_json::from_value(value).context("unexpected response shape from daemon")
    }

    /// Same as `call`, but for requests whose response payload is `null`
    /// (fire-and-forget style commands like `SetRgbEffect`).
    pub async fn call_unit(&self, request: IpcRequest) -> Result<()> {
        self.call_raw(request).await?;
        Ok(())
    }

    async fn call_raw(&self, request: IpcRequest) -> Result<serde_json::Value> {
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
            IpcResponse::Error { message } => bail!("daemon error: {message}"),
        }
    }
}
