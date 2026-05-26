//! Serde mirrors of Reddit's `search.json` response shape.
//!
//! Every optional field is `#[serde(default)]` so a payload that drops a
//! field continues to deserialize. Deserialization failures of *required*
//! fields surface as [`crate::error::TradingError::SchemaViolation`] via
//! the client.

use serde::Deserialize;

/// Top-level Reddit listing response: `{ "kind": "Listing", "data": {...} }`.
#[derive(Debug, Deserialize)]
pub struct RawListing {
    pub data: RawListingData,
}

/// `data` payload of a listing: an array of child wrappers.
#[derive(Debug, Deserialize)]
pub struct RawListingData {
    #[serde(default)]
    pub children: Vec<RawChild>,
}

/// One `{ "kind": "t3", "data": {...} }` child wrapping a submission.
#[derive(Debug, Deserialize)]
pub struct RawChild {
    pub data: RawSubmission,
}

/// A Reddit submission as returned by `search.json`. Only the fields used by
/// [`super::news_provider::RedditNewsProvider`] are listed; unknown fields
/// are ignored (no `#[serde(deny_unknown_fields)]` — see project R29 note).
#[derive(Debug, Deserialize)]
pub struct RawSubmission {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub selftext: String,
    #[serde(default)]
    pub permalink: String,
    #[serde(default)]
    pub subreddit: String,
    /// Unix-seconds creation timestamp. Reddit returns this as `f64`.
    #[serde(default)]
    pub created_utc: f64,
    /// Net upvote score (upvotes − downvotes). May be negative; we clamp at
    /// 0 for `relevance_score` math.
    #[serde(default)]
    pub score: i64,
    #[serde(default)]
    pub over_18: bool,
    #[serde(default)]
    pub stickied: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESPONSE_WITH_ONE_POST: &str = r#"{
        "kind": "Listing",
        "data": {
            "children": [{
                "kind": "t3",
                "data": {
                    "title": "AAPL Q4 thread",
                    "selftext": "discussion body",
                    "permalink": "/r/stocks/comments/abc/aapl_q4/",
                    "subreddit": "stocks",
                    "created_utc": 1713200000.0,
                    "score": 1234,
                    "over_18": false,
                    "stickied": false
                }
            }]
        }
    }"#;

    const RESPONSE_EMPTY: &str = r#"{
        "kind": "Listing",
        "data": { "children": [] }
    }"#;

    const RESPONSE_WITH_UNKNOWN_FIELDS: &str = r#"{
        "kind": "Listing",
        "data": {
            "children": [{
                "kind": "t3",
                "data": {
                    "title": "tolerant",
                    "selftext": "",
                    "permalink": "/r/x/c/y/",
                    "subreddit": "x",
                    "created_utc": 1.0,
                    "score": 50,
                    "over_18": false,
                    "stickied": false,
                    "future_field_we_dont_know_about": 42
                }
            }]
        }
    }"#;

    #[test]
    fn parses_single_post_response() {
        let listing: RawListing = serde_json::from_str(RESPONSE_WITH_ONE_POST).expect("parse");
        assert_eq!(listing.data.children.len(), 1);
        let post = &listing.data.children[0].data;
        assert_eq!(post.title, "AAPL Q4 thread");
        assert_eq!(post.subreddit, "stocks");
        assert_eq!(post.score, 1234);
        assert!(!post.over_18);
    }

    #[test]
    fn parses_empty_listing() {
        let listing: RawListing = serde_json::from_str(RESPONSE_EMPTY).expect("parse");
        assert!(listing.data.children.is_empty());
    }

    #[test]
    fn ignores_unknown_fields_forward_compat() {
        let listing: RawListing =
            serde_json::from_str(RESPONSE_WITH_UNKNOWN_FIELDS).expect("parse");
        assert_eq!(listing.data.children.len(), 1);
    }

    #[test]
    fn missing_optional_fields_default() {
        let json = r#"{
            "kind": "Listing",
            "data": { "children": [ { "kind": "t3", "data": {} } ] }
        }"#;
        let listing: RawListing = serde_json::from_str(json).expect("parse");
        let post = &listing.data.children[0].data;
        assert_eq!(post.title, "");
        assert_eq!(post.score, 0);
        assert!(!post.over_18);
    }
}
