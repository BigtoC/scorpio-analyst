//! Read-only Futu OpenD client (default-off). Talks raw TCP with JSON message
//! bodies; no protobuf, no `build.rs`. See
//! `docs/superpowers/specs/2026-06-01-futu-position-integration-design.md`.

mod frame;

// ── OpenD frame constants ───────────────────────────────────────────────────
// Constants are added alongside the modules that consume them so the per-task
// `-D warnings` gate stays green: handshake/market constants land in Task 6
// (messages), and `ENDPOINT` in Task 8 (client). Defining them all here now
// would trip `dead_code` until those consumers exist.
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
