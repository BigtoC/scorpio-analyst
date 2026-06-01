//! Serde request/response bodies for InitConnect (1001), Trd_GetAccList (2001),
//! and Trd_GetPositionList (2102). Pure: serialize C2S to bytes, parse S2C from
//! bytes. The broker's free-form `retMsg`/`errCode` are never deserialized, so
//! they cannot be surfaced — only the numeric `retType` is reported on error.

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{CLIENT_ID, CLIENT_VER, PACKET_ENC_ALGO_NONE, TRD_ENV_REAL};

/// Accept a `u64` from either a JSON number or a quoted numeric string.
mod flex_u64 {
    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(deserializer: D) -> Result<u64, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum NumOrStr {
            Num(u64),
            Str(String),
        }
        match NumOrStr::deserialize(deserializer)? {
            NumOrStr::Num(n) => Ok(n),
            NumOrStr::Str(s) => s.trim().parse::<u64>().map_err(serde::de::Error::custom),
        }
    }
}

// ── envelope ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct Request<T> {
    c2s: T,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Response<T> {
    ret_type: i32,
    // `Option<T>` is implicitly optional in serde: a missing `s2c` key
    // deserializes to `None` without a `#[serde(default)]` bound on `T`.
    s2c: Option<T>,
}

/// Map a non-success envelope to a sanitized reason. The broker's free-form
/// `retMsg` and `errCode` are intentionally not deserialized, so they can never
/// be surfaced or logged — only the numeric `retType` is reported.
fn check_envelope<T>(resp: &Response<T>, op: &str) -> Result<(), String> {
    if resp.ret_type == 0 {
        return Ok(());
    }
    debug!(op, ret_type = resp.ret_type, "OpenD returned error");
    Err(format!(
        "OpenD {op} returned error (retType {})",
        resp.ret_type
    ))
}

// ── InitConnect 1001 ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct InitConnectC2S {
    client_ver: i32,
    #[serde(rename = "clientID")]
    client_id: String,
    recv_notify: bool,
    packet_enc_algo: i32,
    programming_language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitConnectS2C {
    #[serde(
        rename = "loginUserID",
        default,
        deserialize_with = "flex_u64::deserialize"
    )]
    login_user_id: u64,
}

pub(crate) fn serialize_init_connect() -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: InitConnectC2S {
            client_ver: CLIENT_VER,
            client_id: CLIENT_ID.to_owned(),
            recv_notify: false,
            packet_enc_algo: PACKET_ENC_ALGO_NONE,
            programming_language: "Rust".to_owned(),
        },
    })
    .map_err(|e| format!("OpenD: failed to encode InitConnect request: {e}"))
}

pub(crate) fn parse_init_connect_response(body: &[u8]) -> Result<u64, String> {
    let resp: Response<InitConnectS2C> = serde_json::from_slice(body)
        .map_err(|_| "OpenD: malformed InitConnect response".to_owned())?;
    check_envelope(&resp, "InitConnect")?;
    Ok(resp.s2c.map(|s| s.login_user_id).unwrap_or(0))
}

// ── Trd_GetAccList 2001 ──────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetAccListC2S {
    #[serde(rename = "userID")]
    user_id: u64,
}

/// One account row from `accList`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccListItem {
    pub trd_env: i32,
    #[serde(rename = "accID", deserialize_with = "flex_u64::deserialize")]
    pub acc_id: u64,
    #[serde(default)]
    pub trd_market_auth_list: Vec<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetAccListS2C {
    #[serde(default)]
    acc_list: Vec<AccListItem>,
}

pub(crate) fn serialize_get_acc_list(login_user_id: u64) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: GetAccListC2S {
            user_id: login_user_id,
        },
    })
    .map_err(|e| format!("OpenD: failed to encode GetAccList request: {e}"))
}

pub(crate) fn parse_acc_list_response(body: &[u8]) -> Result<Vec<AccListItem>, String> {
    let resp: Response<GetAccListS2C> = serde_json::from_slice(body)
        .map_err(|_| "OpenD: malformed GetAccList response".to_owned())?;
    check_envelope(&resp, "GetAccList")?;
    Ok(resp.s2c.map(|s| s.acc_list).unwrap_or_default())
}

// ── Trd_GetPositionList 2102 ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TrdHeader {
    trd_env: i32,
    #[serde(rename = "accID")]
    acc_id: u64,
    trd_market: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GetPositionListC2S {
    header: TrdHeader,
    refresh_cache: bool,
}

/// One position row from `positionList`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PositionListItem {
    #[serde(default)]
    pub position_side: i32,
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub qty: f64,
    #[serde(default)]
    pub can_sell_qty: f64,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub cost_price: Option<f64>,
    #[serde(default)]
    pub val: Option<f64>,
    #[serde(default)]
    pub pl_val: Option<f64>,
    #[serde(default)]
    pub pl_ratio: Option<f64>,
    #[serde(default)]
    pub currency: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetPositionListS2C {
    #[serde(default)]
    position_list: Vec<PositionListItem>,
}

pub(crate) fn serialize_get_position_list(acc_id: u64, trd_market: i32) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&Request {
        c2s: GetPositionListC2S {
            header: TrdHeader {
                trd_env: TRD_ENV_REAL,
                acc_id,
                trd_market,
            },
            refresh_cache: false,
        },
    })
    .map_err(|e| format!("OpenD: failed to encode GetPositionList request: {e}"))
}

pub(crate) fn parse_position_list_response(body: &[u8]) -> Result<Vec<PositionListItem>, String> {
    let resp: Response<GetPositionListS2C> = serde_json::from_slice(body)
        .map_err(|_| "OpenD: malformed GetPositionList response".to_owned())?;
    check_envelope(&resp, "GetPositionList")?;
    Ok(resp.s2c.map(|s| s.position_list).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_init_connect_request_with_no_encryption() {
        let body = serialize_init_connect().expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["c2s"]["clientID"], "scorpio-analyst");
        assert_eq!(v["c2s"]["packetEncAlgo"], -1);
        assert_eq!(v["c2s"]["recvNotify"], false);
        assert_eq!(v["c2s"]["clientVer"], 100);
    }

    #[test]
    fn serializes_get_acc_list_request_with_user_id() {
        let body = serialize_get_acc_list(42).expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["c2s"]["userID"], 42);
    }

    #[test]
    fn serializes_position_list_request_with_real_env_and_no_code_filter() {
        let body =
            serialize_get_position_list(987654321, super::super::TRD_MARKET_US).expect("serialize");
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["c2s"]["header"]["trdEnv"], 1); // Real
        assert_eq!(v["c2s"]["header"]["accID"], 987654321_u64);
        assert_eq!(v["c2s"]["header"]["trdMarket"], 2); // US
        assert!(v["c2s"].get("filterConditions").is_none());
        assert_eq!(v["c2s"]["refreshCache"], false);
    }

    #[test]
    fn parses_init_connect_response_and_returns_login_user_id() {
        let body = br#"{"retType":0,"retMsg":"","errCode":0,"s2c":{"connID":7,"loginUserID":555,"keepAliveInterval":10}}"#;
        assert_eq!(parse_init_connect_response(body).unwrap(), 555);
    }

    #[test]
    fn parses_acc_list_with_string_account_id() {
        // accID arrives as a quoted numeric string — must parse to u64.
        let body = br#"{"retType":0,"s2c":{"accList":[{"trdEnv":1,"accID":"281756","trdMarketAuthList":[2]}]}}"#;
        let accounts = parse_acc_list_response(body).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].acc_id, 281756);
        assert_eq!(accounts[0].trd_env, 1);
        assert_eq!(accounts[0].trd_market_auth_list, vec![2]);
    }

    #[test]
    fn parses_position_list_rows() {
        let body = br#"{"retType":0,"s2c":{"positionList":[{"positionSide":0,"code":"AAPL","name":"Apple","qty":100.0,"canSellQty":100.0,"price":185.42,"costPrice":150.0,"val":18542.0,"plVal":3542.0,"plRatio":0.236,"currency":2}]}}"#;
        let rows = parse_position_list_response(body).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].code, "AAPL");
        assert_eq!(rows[0].currency, 2);
        assert_eq!(rows[0].position_side, 0);
    }

    #[test]
    fn non_zero_ret_type_maps_to_sanitized_error_without_raw_retmsg() {
        let body =
            br#"{"retType":-1,"retMsg":"SECRET internal token=abc","errCode":1019,"s2c":null}"#;
        let err = parse_init_connect_response(body).expect_err("non-zero retType must error");
        assert!(err.contains("retType -1"));
        assert!(!err.contains("SECRET"), "raw retMsg must not leak: {err}");
        assert!(!err.contains("abc"));
    }
}
