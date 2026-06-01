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

/// Lower bound for the one-shot lookup timeout. `timeout_secs = 0` would make
/// every enabled lookup time out instantly (a silent footgun), so it is floored.
const FUTU_TIMEOUT_MIN_SECS: u64 = 1;
/// Upper bound: a misconfigured/slowloris OpenD must not stall the analysis past
/// a hard ceiling, even if the operator sets a very large `timeout_secs`.
const FUTU_TIMEOUT_MAX_SECS: u64 = 30;

/// Clamp the configured timeout into `[MIN, MAX]` seconds. Pure and testable.
fn resolve_timeout(secs: u64) -> Duration {
    Duration::from_secs(secs.clamp(FUTU_TIMEOUT_MIN_SECS, FUTU_TIMEOUT_MAX_SECS))
}

/// Read-only Futu OpenD client. Infallible to construct — it only stores config
/// (there is no fallible step; the socket connects lazily per fetch).
#[derive(Debug)]
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
            timeout: resolve_timeout(config.timeout_secs),
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
mod tests {
    use super::*;

    #[test]
    fn resolve_timeout_clamps_zero_up_and_huge_down() {
        assert_eq!(
            resolve_timeout(0),
            Duration::from_secs(FUTU_TIMEOUT_MIN_SECS)
        );
        assert_eq!(resolve_timeout(5), Duration::from_secs(5));
        assert_eq!(
            resolve_timeout(u64::MAX),
            Duration::from_secs(FUTU_TIMEOUT_MAX_SECS)
        );
    }

    #[tokio::test]
    async fn disabled_client_resolves_to_disabled_without_a_socket() {
        let client = FutuClient::new(&FutuConfig::default());
        let symbol = Symbol::parse("AAPL").unwrap();
        assert_eq!(
            client.account_positions(Some(&symbol)).await,
            AccountPositionsState::Disabled
        );
    }

    #[tokio::test]
    async fn enabled_client_without_typed_symbol_is_unavailable_without_a_socket() {
        let cfg = FutuConfig {
            enabled: true,
            account_id: None,
            timeout_secs: 5,
        };
        let client = FutuClient::new(&cfg);
        match client.account_positions(None).await {
            AccountPositionsState::Unavailable(reason) => {
                assert!(reason.contains("no typed symbol"), "got: {reason}")
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    // Covers the one production path the FutuConn trait seam does not: the live
    // socket frame I/O loop (write -> read header -> decode/validate -> read body
    // -> verify SHA-1). A loopback server speaks the frame protocol back.
    #[tokio::test]
    async fn live_conn_request_round_trips_a_framed_response_over_a_socket() {
        // `encode_frame` and `FUTU_HEADER_LEN` come from client's top-level
        // `use super::frame::{...}` via this module's `use super::*`.
        use crate::data::futu::PROTO_INIT_CONNECT;
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut hdr = [0u8; FUTU_HEADER_LEN];
            sock.read_exact(&mut hdr).await.unwrap();
            let body_len = u32::from_le_bytes([hdr[12], hdr[13], hdr[14], hdr[15]]) as usize;
            let mut req_body = vec![0u8; body_len];
            sock.read_exact(&mut req_body).await.unwrap();
            // Respond with serial = 1 (LiveFutuConn's first serial) and matching proto.
            let resp = encode_frame(PROTO_INIT_CONNECT, 1, br#"{"retType":0}"#);
            sock.write_all(&resp).await.unwrap();
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let mut conn = LiveFutuConn::new(stream);
        let body = conn
            .request(PROTO_INIT_CONNECT, br#"{"c2s":{}}"#.to_vec())
            .await
            .expect("request should round-trip");
        assert_eq!(body, br#"{"retType":0}"#);
        server.await.unwrap();
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
        // Print only the resolved shape — never raw holdings — mirroring the
        // "names + types, no values" discipline of examples/futu_opend_smoke.rs.
        match &state {
            AccountPositionsState::Available(snap) => println!(
                "live: Available (market {}, {}, {} position(s))",
                snap.market,
                snap.currency,
                snap.positions.len()
            ),
            AccountPositionsState::Unavailable(reason) => {
                println!("live: Unavailable ({reason})")
            }
            AccountPositionsState::Disabled => panic!("enabled client must not be Disabled"),
        }
    }
}
