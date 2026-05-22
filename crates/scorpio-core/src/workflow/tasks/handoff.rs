//! Private typed handoff between `run_analysis_cycle` and `PreflightTask`.
//!
//! Replaces the prior two-key JSON override transport
//! (`KEY_RUNTIME_POLICY_OVERRIDE` + `KEY_ROUTING_FALLBACK_REASON_OVERRIDE`)
//! with one sealed context value. The struct is `pub(super)` and the
//! accessor functions are `pub(in crate::workflow)`, so no caller outside
//! the workflow module tree names the type.
//!
//! Read path is intentionally string-based + `serde_json::from_str` (not the
//! typed `Context::get::<T>`) because graph-flow's typed `get` returns `None`
//! on any deserialize mismatch, which would silently downgrade to the
//! constructor-derived fallback. The string + explicit parse preserves the
//! fail-loud `TaskExecutionFailed` contract that the old two-key code
//! provided.

use graph_flow::Context;
use serde::{Deserialize, Serialize};

use crate::analysis_packs::RuntimePolicy;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct RuntimePreflightOverride {
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
}

pub(in crate::workflow) const KEY_RUNTIME_PREFLIGHT_OVERRIDE: &str = "runtime_preflight_override";

pub(in crate::workflow) async fn put_into_context(
    context: &Context,
    runtime_policy: RuntimePolicy,
    routing_fallback_reason: Option<String>,
) -> graph_flow::Result<()> {
    let payload = RuntimePreflightOverride {
        runtime_policy,
        routing_fallback_reason,
    };
    let json = serde_json::to_string(&payload).map_err(|err| {
        graph_flow::GraphError::TaskExecutionFailed(format!(
            "orchestration corruption: runtime preflight override serialization failed: {err}"
        ))
    })?;
    context.set(KEY_RUNTIME_PREFLIGHT_OVERRIDE, json).await;
    Ok(())
}

pub(in crate::workflow) async fn try_load_from_context(
    context: &Context,
) -> graph_flow::Result<Option<(RuntimePolicy, Option<String>)>> {
    let raw: Option<String> = context.get(KEY_RUNTIME_PREFLIGHT_OVERRIDE).await;
    let Some(json) = raw else {
        return Ok(None);
    };
    let payload: RuntimePreflightOverride = serde_json::from_str(&json).map_err(|err| {
        graph_flow::GraphError::TaskExecutionFailed(format!(
            "PreflightTask: orchestration corruption: runtime preflight override deserialization failed: {err}"
        ))
    })?;
    Ok(Some((
        payload.runtime_policy,
        payload.routing_fallback_reason,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis_packs::resolve_runtime_policy;

    fn baseline_policy() -> RuntimePolicy {
        resolve_runtime_policy("baseline").expect("baseline pack must resolve")
    }

    #[tokio::test]
    async fn roundtrip_preserves_policy_and_reason() {
        let context = Context::new();
        let policy = baseline_policy();
        put_into_context(
            &context,
            policy.clone(),
            Some("profile_lookup_unavailable".to_owned()),
        )
        .await
        .expect("write");
        let (loaded_policy, loaded_reason) = try_load_from_context(&context)
            .await
            .expect("read")
            .expect("override present");
        assert_eq!(loaded_policy, policy);
        assert_eq!(loaded_reason.as_deref(), Some("profile_lookup_unavailable"));
    }

    #[tokio::test]
    async fn absent_key_returns_ok_none() {
        let context = Context::new();
        let outcome = try_load_from_context(&context).await.expect("read");
        assert!(outcome.is_none());
    }

    #[tokio::test]
    async fn malformed_payload_returns_task_execution_failed() {
        let context = Context::new();
        context
            .set(KEY_RUNTIME_PREFLIGHT_OVERRIDE, "{not valid json".to_owned())
            .await;
        let err = try_load_from_context(&context)
            .await
            .expect_err("malformed override must surface as TaskExecutionFailed");
        match err {
            graph_flow::GraphError::TaskExecutionFailed(message) => {
                assert!(
                    message.contains("runtime preflight override"),
                    "error message should identify the override subsystem: {message}"
                );
            }
            other => panic!("expected TaskExecutionFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn absent_reason_roundtrips_as_none() {
        let context = Context::new();
        put_into_context(&context, baseline_policy(), None)
            .await
            .expect("write");
        let (_, reason) = try_load_from_context(&context)
            .await
            .expect("read")
            .expect("override present");
        assert!(reason.is_none());
    }
}
