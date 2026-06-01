//! Public `FutuClient` entry point + the live `TcpStream` transport.

use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::debug;

use super::frame::{
    FUTU_HEADER_LEN, decode_header, encode_frame, validate_response_header, verify_body_sha1,
};
use super::{ENDPOINT, FutuConn, fetch_account_snapshot};
use crate::config::FutuConfig;
use crate::domain::Symbol;
use crate::state::AccountPositionsState;

/// Read-only Futu OpenD client. Infallible to construct — it only stores config
/// (there is no fallible step; the socket connects lazily per fetch).
pub struct FutuClient {
    enabled: bool,
    account_id: Option<u64>,
    timeout: Duration,
}

impl FutuClient {
    #[must_use]
    pub fn new(config: &FutuConfig) -> Self {
        Self {
            enabled: config.enabled,
            account_id: config.account_id,
            timeout: Duration::from_secs(config.timeout_secs),
        }
    }

    /// Resolve the three-state account-positions contract for `symbol`. Never
    /// returns `Err`: disabled → `Disabled`; any failure → `Unavailable(reason)`.
    pub async fn account_positions(&self, symbol: Option<&Symbol>) -> AccountPositionsState {
        if !self.enabled {
            return AccountPositionsState::Disabled;
        }
        let Some(symbol) = symbol else {
            return AccountPositionsState::Unavailable(
                "account positions: no typed symbol for lookup".to_owned(),
            );
        };
        match self.fetch(symbol).await {
            Ok(snapshot) => AccountPositionsState::Available(snapshot),
            Err(reason) => {
                debug!(reason = %reason, "account positions unavailable");
                AccountPositionsState::Unavailable(reason)
            }
        }
    }

    async fn fetch(&self, symbol: &Symbol) -> Result<crate::state::AccountSnapshot, String> {
        let work = async {
            let stream = TcpStream::connect(ENDPOINT)
                .await
                .map_err(|_| format!("OpenD unreachable on {ENDPOINT}"))?;
            let mut conn = LiveFutuConn::new(stream);
            fetch_account_snapshot(&mut conn, symbol, self.account_id).await
        };
        match tokio::time::timeout(self.timeout, work).await {
            Ok(result) => result,
            Err(_) => Err(format!(
                "OpenD lookup timed out after {}s",
                self.timeout.as_secs()
            )),
        }
    }
}

/// Live framed transport over a single one-shot TCP connection.
struct LiveFutuConn {
    stream: TcpStream,
    serial: u32,
}

impl LiveFutuConn {
    fn new(stream: TcpStream) -> Self {
        Self { stream, serial: 0 }
    }
}

#[async_trait]
impl FutuConn for LiveFutuConn {
    async fn request(&mut self, proto_id: u32, body: Vec<u8>) -> Result<Vec<u8>, String> {
        self.serial = self.serial.wrapping_add(1);
        let frame = encode_frame(proto_id, self.serial, &body);
        self.stream
            .write_all(&frame)
            .await
            .map_err(|_| "OpenD: write failed".to_owned())?;

        let mut header_bytes = [0u8; FUTU_HEADER_LEN];
        self.stream
            .read_exact(&mut header_bytes)
            .await
            .map_err(|_| "OpenD: failed to read response header".to_owned())?;
        let header = decode_header(&header_bytes)?;
        validate_response_header(&header, proto_id, self.serial)?;

        let mut body_buf = vec![0u8; header.body_len as usize];
        self.stream
            .read_exact(&mut body_buf)
            .await
            .map_err(|_| "OpenD: failed to read response body".to_owned())?;
        verify_body_sha1(&header, &body_buf)?;
        Ok(body_buf)
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;
    use crate::config::FutuConfig;

    /// Manual connectivity spike. Requires a running OpenD on 127.0.0.1:11111
    /// with API encryption disabled. Prints the resolved state so the operator
    /// can confirm JSON mode, the no-encryption handshake, and field casing.
    /// Sanitize any captured payloads before turning them into fixtures.
    #[tokio::test]
    #[ignore = "requires live Futu OpenD on 127.0.0.1:11111 — run manually"]
    async fn futu_live_account_positions_smoke() {
        let cfg = FutuConfig {
            enabled: true,
            account_id: None,
            timeout_secs: 10,
        };
        let client = FutuClient::new(&cfg);
        let symbol = Symbol::parse("AAPL").unwrap();
        let state = client.account_positions(Some(&symbol)).await;
        // Print, don't hard-assert specific holdings — this is a capture spike.
        println!("live account positions: {state:#?}");
        match state {
            AccountPositionsState::Available(_) | AccountPositionsState::Unavailable(_) => {}
            AccountPositionsState::Disabled => panic!("enabled client must not be Disabled"),
        }
    }
}
