//! Read-only Futu OpenD client (default-off). Talks raw TCP with JSON message
//! bodies; no protobuf, no `build.rs`. See
//! `docs/superpowers/specs/2026-06-01-futu-position-integration-design.md`.

mod client;
mod frame;
mod messages;
mod select;

use async_trait::async_trait;

pub use client::FutuClient;

use crate::domain::Symbol;
use crate::state::AccountSnapshot;
use messages::{
    parse_acc_list_response, parse_init_connect_response, parse_position_list_response,
    serialize_get_acc_list, serialize_get_position_list, serialize_init_connect,
};
use select::{assemble_snapshot, market_for_symbol, select_account};

// ── OpenD protocol constants ────────────────────────────────────────────────
// Grouped here and consumed by the submodules: frame-header fields by `frame`,
// handshake/market codes by `messages`, and `ENDPOINT` by `client`.
/// `nProtoFmtType` = 1 (JSON body).
pub(crate) const PROTO_FMT_JSON: u8 = 1;
/// `nProtoVer` = 0.
pub(crate) const PROTO_VER: u8 = 0;
/// Reject any response body larger than this (DoS guard).
pub(crate) const MAX_BODY_LEN: u32 = 16 * 1024 * 1024;

/// `InitConnect` protocol id.
pub(crate) const PROTO_INIT_CONNECT: u32 = 1001;
/// `Trd_GetAccList` protocol id.
pub(crate) const PROTO_GET_ACC_LIST: u32 = 2001;
/// `Trd_GetPositionList` protocol id.
pub(crate) const PROTO_GET_POSITION_LIST: u32 = 2102;

/// Hardcoded Real trading environment (`TrdEnv_Real`). Used by the account
/// filter and the `TrdHeader`. There is no paper-account mode in v1.
pub(crate) const TRD_ENV_REAL: i32 = 1;
/// `TrdMarket_US`.
pub(crate) const TRD_MARKET_US: i32 = 2;
/// No-encryption `packetEncAlgo` (PacketEncAlgo_None). Confirmed accepted by a
/// live OpenD InitConnect in the Task 0 spike.
pub(crate) const PACKET_ENC_ALGO_NONE: i32 = -1;
/// `clientID` sent in `InitConnect`.
pub(crate) const CLIENT_ID: &str = "scorpio-analyst";
/// `clientVer` sent in `InitConnect`. Accepted by the live OpenD in the Task 0
/// spike; raise it if OpenD ever reports "client version too low".
pub(crate) const CLIENT_VER: i32 = 100;

/// Hardcoded local OpenD endpoint (remote hosts are an explicit non-goal while
/// encryption is unsupported). Consumed by `client.rs`.
pub(crate) const ENDPOINT: &str = "127.0.0.1:11111";

/// One-method transport seam over a framed OpenD connection. The live impl
/// (`client::LiveFutuConn`) handles framing + SHA-1 + the socket; tests script
/// canned response bodies via `MockFutuConn`. Mirrors the `EdgarHttp` seam.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub(crate) trait FutuConn: Send {
    /// Frame `body` under `proto_id`, send it, read the framed response, and
    /// return the raw response body bytes. Transport/framing errors return a
    /// sanitized `Err(String)`.
    async fn request(&mut self, proto_id: u32, body: Vec<u8>) -> Result<Vec<u8>, String>;
}

/// Drive the read-only sequence: InitConnect → GetAccList → (pick account) →
/// GetPositionList → assemble. Any step's `Err` short-circuits (later requests
/// are not sent). Returns a sanitized reason on failure.
pub(crate) async fn fetch_account_snapshot<C: FutuConn + ?Sized>(
    conn: &mut C,
    symbol: &Symbol,
    account: Option<&str>,
) -> Result<AccountSnapshot, String> {
    let market = market_for_symbol(symbol)?;

    let login_user_id = parse_init_connect_response(
        &conn
            .request(PROTO_INIT_CONNECT, serialize_init_connect()?)
            .await?,
    )?;

    let accounts = parse_acc_list_response(
        &conn
            .request(PROTO_GET_ACC_LIST, serialize_get_acc_list(login_user_id)?)
            .await?,
    )?;

    let acc_id = select_account(&accounts, market, account)?;

    let rows = parse_position_list_response(
        &conn
            .request(
                PROTO_GET_POSITION_LIST,
                serialize_get_position_list(acc_id, market)?,
            )
            .await?,
    )?;

    Ok(assemble_snapshot(acc_id, rows, market))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Symbol;
    use mockall::Sequence;

    fn ok_init() -> Vec<u8> {
        br#"{"retType":0,"s2c":{"loginUserID":555}}"#.to_vec()
    }
    fn ok_accounts() -> Vec<u8> {
        br#"{"retType":0,"s2c":{"accList":[{"trdEnv":1,"accID":281756,"trdMarketAuthList":[2]}]}}"#
            .to_vec()
    }
    fn ok_positions() -> Vec<u8> {
        br#"{"retType":0,"s2c":{"positionList":[{"positionSide":0,"code":"US.AAPL","name":"Apple","qty":100.0,"canSellQty":100.0,"price":185.42,"costPrice":150.0,"val":18542.0,"plVal":3542.0,"plRatio":23.6,"currency":2}]}}"#.to_vec()
    }

    #[tokio::test]
    async fn fetch_runs_init_then_acclist_then_positionlist_in_order() {
        let mut conn = MockFutuConn::new();
        let mut seq = Sequence::new();
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|proto, _| *proto == PROTO_INIT_CONNECT)
            .returning(|_, _| Ok(ok_init()));
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|proto, _| *proto == PROTO_GET_ACC_LIST)
            .returning(|_, _| Ok(ok_accounts()));
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|proto, body| {
                // GetPositionList must carry Real env and the selected accID, without a code filter.
                *proto == PROTO_GET_POSITION_LIST && {
                    let v: serde_json::Value = serde_json::from_slice(body).unwrap();
                    v["c2s"]["header"]["trdEnv"] == 1
                        && v["c2s"]["header"]["accID"] == 281756_u64
                        && v["c2s"].get("filterConditions").is_none()
                        && v["c2s"]["refreshCache"] == false
                }
            })
            .returning(|_, _| Ok(ok_positions()));

        let symbol = Symbol::parse("AAPL").unwrap();
        let snap = fetch_account_snapshot(&mut conn, &symbol, None)
            .await
            .expect("snapshot");
        assert_eq!(snap.positions.len(), 1);
        assert_eq!(snap.held_position(&symbol).unwrap().code, "AAPL");
    }

    #[tokio::test]
    async fn fetch_short_circuits_when_init_fails() {
        let mut conn = MockFutuConn::new();
        conn.expect_request()
            .times(1)
            .withf(|proto, _| *proto == PROTO_INIT_CONNECT)
            .returning(|_, _| Err("connection refused".to_owned()));
        // No further calls expected — GetAccList/GetPositionList must not be sent.

        let symbol = Symbol::parse("AAPL").unwrap();
        let err = fetch_account_snapshot(&mut conn, &symbol, None)
            .await
            .expect_err("init failure must propagate");
        assert!(err.contains("connection refused"));
    }

    #[tokio::test]
    async fn fetch_is_unavailable_when_no_real_account_matches_market() {
        let mut conn = MockFutuConn::new();
        let mut seq = Sequence::new();
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| Ok(ok_init()));
        conn.expect_request()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _| {
                Ok(
                    br#"{"retType":0,"s2c":{"accList":[{"trdEnv":0,"accID":1,"trdMarketAuthList":[2]}]}}"#
                        .to_vec(),
                )
            });
        // Position fetch must not happen — no real account.

        let symbol = Symbol::parse("AAPL").unwrap();
        let err = fetch_account_snapshot(&mut conn, &symbol, None)
            .await
            .expect_err("no real account");
        assert!(err.contains("no real account"));
    }
}
