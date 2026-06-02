//! Live Futu OpenD protocol spike (sanitized diagnostic).
//!
//! **NOT run automatically in CI** — `cargo nextest` does not execute `examples/`.
//! Run manually against a local OpenD with API encryption disabled:
//!
//! ```sh
//! cargo run -p scorpio-core --example futu_opend_smoke
//! ```
//!
//! Validates the assumptions the `data::futu` implementation depends on, before
//! that code exists. It deliberately duplicates a minimal copy of the frame
//! encode/decode logic so it does not depend on the later modules.
//!
//! Privacy: this script prints account ids in decimal (so the operator can copy
//! the one to configure) plus a redacted `acct-<hash>` label, but **never** the
//! raw holdings (quantities/codes/names) or raw broker error text — only field
//! names and JSON types for position rows. `loginUserID` is reported only as
//! zero/non-zero. Sanitize anything captured here before turning it into a test
//! fixture.
//!
//! Answers:
//! - Does JSON mode (`nProtoFmtType = 1`) work for 1001 / 2001 / 2102?
//! - Is `packetEncAlgo = -1` accepted?
//! - Does `GetAccList` need the real `loginUserID`, or does `0` work?
//! - How are `accID` / `plRatio` / casing represented on the wire?
//! - Does omitting `filterConditions.codeList` return the full position list?
//! - With `SCORPIO__FUTU__ACCOUNT` set, does the `uniCardNum` / `accID` lookup
//!   (mirroring `select_account`) resolve a Real US account?
//!
//! Findings validated against a live OpenD + the bundled Futu proto
//! (`futu/common/pb/Trd_Common.proto`, `Trd_GetPositionList.proto`):
//! - JSON mode and `packetEncAlgo = -1` accepted for 1001/2001/2102.
//! - `GetAccList` works with `userID = 0`; `loginUserID` is returned as a
//!   *string* (handled by `flex_u64` in `messages.rs`).
//! - `accID` is a wire *string* but OpenD accepts it back as a number in the
//!   `TrdHeader` (`GetPositionList` returned `retType = 0`).
//! - `plRatio` is a **percentage**, not a fraction: the proto states
//!   "plRatio 等于 8.8 代表涨 8.8%". `assemble_snapshot` divides by 100 to store a
//!   fraction (the render/report code multiplies back by 100). See plan
//!   decision #3.
//! - Omitting `filterConditions` returns the full account/market position list.
//! - Enums: PositionSide Long=0/Short=1; TrdEnv Real=1; TrdMarket US=2;
//!   Currency USD=2 — all match the plan's label tables.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const ENDPOINT: &str = "127.0.0.1:11111";
const HEADER_LEN: usize = 44;
const PROTO_INIT_CONNECT: u32 = 1001;
const PROTO_GET_ACC_LIST: u32 = 2001;
const PROTO_GET_POSITION_LIST: u32 = 2102;
const TRD_ENV_REAL: i64 = 1;
const TRD_MARKET_US: i64 = 2;

/// Encode a 44-byte OpenD frame header + body. Layout (little-endian):
/// `FT`(2) protoID(4) protoFmt(1=JSON) protoVer(1=0) serial(4) bodyLen(4)
/// bodySHA1(20) reserved(8).
fn encode_frame(proto_id: u32, serial: u32, body: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + body.len());
    buf.extend_from_slice(b"FT");
    buf.extend_from_slice(&proto_id.to_le_bytes());
    buf.push(1); // nProtoFmtType = JSON
    buf.push(0); // nProtoVer
    buf.extend_from_slice(&serial.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    let mut hasher = Sha1::new();
    hasher.update(body);
    let digest: [u8; 20] = hasher.finalize().into();
    buf.extend_from_slice(&digest);
    buf.extend_from_slice(&[0u8; 8]);
    buf.extend_from_slice(body);
    buf
}

/// Send one framed request and return the raw response body bytes.
async fn request(
    stream: &mut TcpStream,
    serial: u32,
    proto_id: u32,
    body: &[u8],
) -> Result<Vec<u8>, String> {
    let frame = encode_frame(proto_id, serial, body);
    stream
        .write_all(&frame)
        .await
        .map_err(|e| format!("write failed: {e}"))?;

    let mut header = [0u8; HEADER_LEN];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|e| format!("read header failed: {e}"))?;
    if &header[0..2] != b"FT" {
        return Err("bad response magic".to_owned());
    }
    let resp_proto = u32::from_le_bytes([header[2], header[3], header[4], header[5]]);
    let body_len = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
    if resp_proto != proto_id {
        println!("    ! response protoID {resp_proto} != request {proto_id}");
    }
    let mut body_buf = vec![0u8; body_len];
    stream
        .read_exact(&mut body_buf)
        .await
        .map_err(|e| format!("read body failed: {e}"))?;
    Ok(body_buf)
}

/// JSON type tag for a value (no value content leaked).
fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) => {
            if n.is_f64() && n.as_i64().is_none() {
                "number(float)"
            } else {
                "number(int)"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Print each key of an object with its JSON type — names + types, no values.
fn print_object_shape(indent: &str, obj: &Value) {
    if let Some(map) = obj.as_object() {
        for (k, v) in map {
            println!("{indent}{k}: {}", type_of(v));
        }
    }
}

/// The raw account id as a decimal string (wire `accID` is a string or number).
/// Printed in this manual diagnostic so the operator can copy the exact id into
/// `SCORPIO__FUTU__ACCOUNT` / the setup wizard. Holdings stay redacted.
fn raw_acc_id(acc: &Value) -> String {
    match acc {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => "?".to_owned(),
    }
}

/// Which identifier (if any) the configured selector matches on this account.
/// Mirrors `select_account`'s lookup in `data::futu::select`: `uniCardNum`
/// (universal account number shown in the Futu app) or raw `accID`.
fn matched_identifier(acc: &Value, wanted: &str) -> Option<&'static str> {
    if acc["uniCardNum"].as_str() == Some(wanted) {
        Some("uniCardNum")
    } else if raw_acc_id(&acc["accID"]) == wanted {
        Some("accID")
    } else {
        None
    }
}

/// Redact an account id (string or number) to a short non-reversible label.
fn redact_acc_id(acc: &Value) -> String {
    let raw = match acc {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => "?".to_owned(),
    };
    let mut hasher = Sha1::new();
    hasher.update(raw.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(3).map(|b| format!("{b:02x}")).collect();
    format!("acct-{hex}")
}

#[tokio::main]
async fn main() {
    println!("Futu OpenD protocol spike → {ENDPOINT}");
    println!("(sanitized: field names/types only; no raw ids/holdings)\n");

    let mut stream = match TcpStream::connect(ENDPOINT).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("FAIL: cannot connect to {ENDPOINT}: {e}");
            eprintln!("Is OpenD running with API encryption disabled?");
            std::process::exit(1);
        }
    };
    let mut serial = 0u32;
    let mut next = || {
        serial += 1;
        serial
    };

    // ── 1001 InitConnect (packetEncAlgo = -1, JSON) ──────────────────────────
    println!("== InitConnect (1001) ==");
    let init_body = json!({
        "c2s": {
            "clientVer": 100,
            "clientID": "scorpio-analyst",
            "recvNotify": false,
            "packetEncAlgo": -1,
            "programmingLanguage": "Rust"
        }
    });
    let init_bytes = serde_json::to_vec(&init_body).unwrap();
    let login_user_id: i64 =
        match request(&mut stream, next(), PROTO_INIT_CONNECT, &init_bytes).await {
            Ok(body) => match serde_json::from_slice::<Value>(&body) {
                Ok(v) => {
                    let ret = v["retType"].as_i64().unwrap_or(-999);
                    println!("  retType = {ret}  (0 = OK)");
                    println!("  JSON mode accepted: {}", ret == 0);
                    println!("  packetEncAlgo=-1 accepted: {}", ret == 0);
                    if let Some(s2c) = v.get("s2c").filter(|s| s.is_object()) {
                        println!("  s2c shape:");
                        print_object_shape("    ", s2c);
                        let uid = s2c["loginUserID"].as_i64().unwrap_or(0);
                        println!(
                            "  loginUserID present: {}, non-zero: {}",
                            s2c.get("loginUserID").is_some(),
                            uid != 0
                        );
                        uid
                    } else {
                        println!("  ! no s2c object on InitConnect");
                        0
                    }
                }
                Err(e) => {
                    eprintln!("  FAIL parse InitConnect: {e}");
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("  FAIL InitConnect: {e}");
                std::process::exit(1);
            }
        };

    // ── 2001 GetAccList: does userID=0 work, or is the real loginUserID needed? ─
    println!("\n== GetAccList (2001) ==");
    let acc_list = probe_acc_list(&mut stream, &mut next, login_user_id).await;

    // Inspect account rows: shape + types, redacted ids, find a Real US account.
    // With SCORPIO__FUTU__ACCOUNT set, the account is instead resolved by
    // uniCardNum / accID — same lookup as `select_account` — to validate the
    // selector against live wire data.
    let wanted_account = std::env::var("SCORPIO__FUTU__ACCOUNT")
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let mut chosen_acc: Option<Value> = None;
    if let Some(first) = acc_list.first() {
        println!("  accList[0] shape:");
        print_object_shape("    ", first);
        println!(
            "  accID json type: {}",
            type_of(&first.get("accID").cloned().unwrap_or(Value::Null))
        );
    }
    println!("  account rows: {}", acc_list.len());
    for acc in &acc_list {
        let env = acc["trdEnv"].as_i64().unwrap_or(-1);
        let markets: Vec<i64> = acc["trdMarketAuthList"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        let uni_card_num = acc["uniCardNum"].as_str().unwrap_or("<none>");
        println!(
            "    accID={} ({}) uniCardNum={uni_card_num} trdEnv={env} ({}) markets={markets:?}",
            raw_acc_id(&acc["accID"]),
            redact_acc_id(&acc["accID"]),
            if env == TRD_ENV_REAL {
                "REAL"
            } else {
                "non-real"
            }
        );
        let selectable = env == TRD_ENV_REAL && markets.contains(&TRD_MARKET_US);
        match &wanted_account {
            Some(wanted) => {
                if let Some(field) = matched_identifier(acc, wanted) {
                    if selectable && chosen_acc.is_none() {
                        println!("      ↑ matches SCORPIO__FUTU__ACCOUNT via {field}");
                        chosen_acc = Some(acc.clone());
                    } else if !selectable {
                        // Near-miss: identifier matched but the account fails the
                        // Real + US-authorized gate `select_account` applies too.
                        let reason = if env != TRD_ENV_REAL {
                            "not a Real account".to_owned()
                        } else {
                            format!(
                                "not authorized for US (markets={markets:?}, need {TRD_MARKET_US})"
                            )
                        };
                        println!(
                            "      ↑ matches SCORPIO__FUTU__ACCOUNT via {field}, but skipped: {reason}"
                        );
                    }
                }
            }
            None => {
                if chosen_acc.is_none() && selectable {
                    chosen_acc = Some(acc.clone());
                }
            }
        }
    }
    if wanted_account.is_some() && chosen_acc.is_none() {
        println!(
            "  ! SCORPIO__FUTU__ACCOUNT is set but no Real US account matched it by uniCardNum/accID"
        );
    }

    // ── 2102 GetPositionList for the chosen Real US account, no code filter ───
    println!("\n== GetPositionList (2102) ==");
    let Some(acc) = chosen_acc else {
        println!("  (no Real account authorized for US market — cannot probe positions)");
        println!("\nDone.");
        return;
    };
    println!(
        "  using accID={} ({}) (Real, US)",
        raw_acc_id(&acc["accID"]),
        redact_acc_id(&acc["accID"])
    );

    // accID must be sent back in the same representation OpenD used. The wire
    // header field is a uint64; if accID arrived as a string, parse it.
    let acc_id_num: u64 = match &acc["accID"] {
        Value::String(s) => s.trim().parse().unwrap_or(0),
        Value::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    };
    let pos_body = json!({
        "c2s": {
            "header": { "trdEnv": TRD_ENV_REAL, "accID": acc_id_num, "trdMarket": TRD_MARKET_US },
            "refreshCache": false
        }
    });
    let pos_bytes = serde_json::to_vec(&pos_body).unwrap();
    let us_rows = match request(&mut stream, next(), PROTO_GET_POSITION_LIST, &pos_bytes).await {
        Ok(body) => match serde_json::from_slice::<Value>(&body) {
            Ok(v) => {
                let ret = v["retType"].as_i64().unwrap_or(-999);
                println!("  retType = {ret}  (0 = OK)");
                let rows = v["s2c"]["positionList"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                println!(
                    "  omitting filterConditions returned {} US position row(s)",
                    rows.len()
                );
                rows
            }
            Err(e) => {
                eprintln!("  FAIL parse GetPositionList: {e}");
                Vec::new()
            }
        },
        Err(e) => {
            eprintln!("  FAIL GetPositionList: {e}");
            Vec::new()
        }
    };

    // ── Shape capture: find ANY Real account with positions, dump sanitized ──
    // shape. The US account is the one the v1 implementation uses, but it may be
    // empty; the position-row field casing + plRatio units are the same across
    // markets, so any non-empty Real account validates Task 6.
    println!("\n== Position-row shape capture (any Real account) ==");
    if !us_rows.is_empty() {
        report_position_shape(&us_rows);
    } else {
        let mut captured = false;
        for acc in &acc_list {
            if acc["trdEnv"].as_i64().unwrap_or(-1) != TRD_ENV_REAL {
                continue;
            }
            let market = acc["trdMarketAuthList"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(Value::as_i64)
                .unwrap_or(0);
            let acc_id_num: u64 = match &acc["accID"] {
                Value::String(s) => s.trim().parse().unwrap_or(0),
                Value::Number(n) => n.as_u64().unwrap_or(0),
                _ => 0,
            };
            let body = serde_json::to_vec(&json!({
                "c2s": {
                    "header": { "trdEnv": TRD_ENV_REAL, "accID": acc_id_num, "trdMarket": market },
                    "refreshCache": false
                }
            }))
            .unwrap();
            if let Ok(resp) = request(&mut stream, next(), PROTO_GET_POSITION_LIST, &body).await
                && let Ok(v) = serde_json::from_slice::<Value>(&resp)
            {
                let rows = v["s2c"]["positionList"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !rows.is_empty() {
                    println!(
                        "  accID={} ({}) (market={market}) has {} position row(s)",
                        raw_acc_id(&acc["accID"]),
                        redact_acc_id(&acc["accID"]),
                        rows.len()
                    );
                    report_position_shape(&rows);
                    captured = true;
                    break;
                }
            }
        }
        if !captured {
            println!("  (no Real account held any positions — row shape not captured)");
        }
    }

    println!("\nDone. Reconcile Task 5/6 constants against the shapes above.");
}

/// Print a sanitized position-row shape: field names + JSON types, the union of
/// keys across rows, and the `plRatio` scale bucket. No codes/names/quantities/
/// values are printed.
fn report_position_shape(rows: &[Value]) {
    let Some(first) = rows.first() else { return };
    println!("  positionList[0] shape (names + types, no values):");
    print_object_shape("    ", first);
    if let Some(pr) = first.get("plRatio").and_then(Value::as_f64) {
        println!(
            "  plRatio scale looks like: {}",
            if pr.abs() < 1.5 {
                "FRACTION (0.236 = 23.6%)"
            } else {
                "PERCENT (23.6 = 23.6%)"
            }
        );
    }
    if let Some(side) = first.get("positionSide") {
        println!("  positionSide json type: {}", type_of(side));
    }
    if let Some(cur) = first.get("currency") {
        println!("  currency json type: {}", type_of(cur));
    }
    let mut all_keys: BTreeSet<String> = BTreeSet::new();
    for r in rows {
        if let Some(m) = r.as_object() {
            all_keys.extend(m.keys().cloned());
        }
    }
    println!("  union of position keys: {all_keys:?}");
}

/// Try `GetAccList` with `userID = 0` first; if that errors or returns no
/// accounts, retry with the real `loginUserID`. Reports which form worked.
async fn probe_acc_list(
    stream: &mut TcpStream,
    next: &mut impl FnMut() -> u32,
    login_user_id: i64,
) -> Vec<Value> {
    for (label, uid) in [("userID=0", 0i64), ("userID=loginUserID", login_user_id)] {
        if uid == login_user_id && login_user_id == 0 && label.contains("loginUserID") {
            continue; // same as the 0 probe; skip
        }
        let body = serde_json::to_vec(&json!({ "c2s": { "userID": uid } })).unwrap();
        match request(stream, next(), PROTO_GET_ACC_LIST, &body).await {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => {
                    let ret = v["retType"].as_i64().unwrap_or(-999);
                    let accs = v["s2c"]["accList"].as_array().cloned().unwrap_or_default();
                    println!("  [{label}] retType={ret} accounts={}", accs.len());
                    if ret == 0 && !accs.is_empty() {
                        println!("  → GetAccList works with {label}");
                        return accs;
                    }
                }
                Err(e) => println!("  [{label}] parse error: {e}"),
            },
            Err(e) => println!("  [{label}] request error: {e}"),
        }
    }
    println!("  → GetAccList returned no accounts with either userID form");
    Vec::new()
}
