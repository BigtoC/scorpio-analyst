---
title: Setup config provider override precedence and compatibility regression
date: 2026-04-30
category: docs/solutions/logic-errors
module: setup-config
problem_type: logic_error
component: tooling
symptoms:
  - `load_user_config_at` dropped provider `base_url` and `rpm` overrides from legacy flat user-config keys
  - setup provider-only loading lost file-backed `[providers.*]` overrides when the in-memory partial had `None`
  - runtime provider loading could clear `SCORPIO__PROVIDERS__...` env overrides after deserialization
root_cause: config_error
resolution_type: code_fix
severity: high
tags:
  - rust
  - setup
  - config
  - provider-overrides
  - precedence
  - backward-compatibility
  - serde
---

# Setup config provider override precedence and compatibility regression

## Problem

The setup/config refactor added nested `[providers.*]` serialization for provider `base_url` and `rpm` overrides, but the new load/merge path changed behavior in two bad ways. Existing user-config files with legacy flat keys stopped round-tripping correctly, and later merge steps overwrote file or env provider overrides with empty in-memory partial values.

## Symptoms

- `settings::tests::load_user_config_at_preserves_provider_overrides` failed because `openai_base_url` and `deepseek_base_url` loaded as `None`.
- `config::tests::load_effective_providers_config_from_user_path_preserves_file_provider_overrides_while_ignoring_stale_copilot_routing` failed because file-backed `[providers.deepseek]` overrides were erased.
- A targeted regression test showed `Config::load_effective_runtime(...)` could deserialize `SCORPIO__PROVIDERS__DEEPSEEK__BASE_URL`, then immediately clear it back to `None`.

## What Didn't Work

- Treating the failures as isolated test updates was wrong. The tests were exposing a real persisted-config compatibility break.
- Keeping `apply_partial_provider_overrides(...)` as a post-deserialize step was wrong for non-secret values. Once config/env layering had already resolved precedence, replaying the partial afterward could only destroy information.

## Solution

Make the settings boundary load both shapes, but only write the canonical nested one.

In `crates/scorpio-core/src/settings.rs`, extend `UserConfigFile` to accept both nested provider tables and legacy flat fields, then prefer nested values when both are present:

```rust
impl From<UserConfigFile> for PartialConfig {
    fn from(value: UserConfigFile) -> Self {
        let openai = value.providers.openai;
        let deepseek = value.providers.deepseek;

        Self {
            openai_base_url: openai.base_url.or(value.openai_base_url),
            openai_rpm: openai.rpm.or(value.openai_rpm),
            deepseek_base_url: deepseek.base_url.or(value.deepseek_base_url),
            deepseek_rpm: deepseek.rpm.or(value.deepseek_rpm),
            // ...other fields omitted
        }
    }
}
```

Keep writes canonical by serializing only the nested provider tables:

```rust
impl From<&PartialConfig> for UserConfigFile {
    fn from(value: &PartialConfig) -> Self {
        Self {
            providers: UserConfigProviders {
                openai: UserConfigProvider {
                    base_url: value.openai_base_url.clone(),
                    rpm: value.openai_rpm,
                },
                deepseek: UserConfigProvider {
                    base_url: value.deepseek_base_url.clone(),
                    rpm: value.deepseek_rpm,
                },
                // ...other providers omitted
            },
            openai_base_url: None,
            openai_rpm: None,
            deepseek_base_url: None,
            deepseek_rpm: None,
            // ...other fields omitted
        }
    }
}
```

For runtime/provider loading in `crates/scorpio-core/src/config.rs`, push partial non-secret overrides into the config source stack instead of replaying them destructively after deserialization:

```rust
let partial_toml = partial_to_nested_toml_non_secrets(partial)?;

let settings = config::Config::builder()
    .add_source(config::File::from(path.as_ref()).required(false))
    .add_source(config::File::from_str(&partial_toml, config::FileFormat::Toml).required(false))
    .add_source(
        config::Environment::with_prefix("SCORPIO")
            .separator("__")
            .try_parsing(true),
    )
    .build()?;
```

And in the full runtime path, remove the extra post-deserialize non-secret override step entirely:

```rust
let mut cfg: Config = config::Config::builder()
    .add_source(config::File::from_str(&nested_toml, config::FileFormat::Toml).required(false))
    .add_source(config::Environment::with_prefix("SCORPIO").separator("__").try_parsing(true))
    .build()?
    .try_deserialize()?;

// Do not replay non-secret provider overrides here.
// Only inject secrets after deserialization.
```

Regression coverage added:

- `load_user_config_at_prefers_nested_provider_overrides_over_legacy_flat_keys`
- `load_effective_runtime_uses_env_provider_base_url_override_over_partial_override`
- `load_effective_providers_config_from_user_path_uses_env_base_url_override_over_partial_override`

## Why This Works

The real problem was precedence being implemented twice in different ways.

- The `config` crate already knows how to merge sources in the right order: file, then partial overlay, then env.
- Reapplying `PartialConfig` non-secrets afterward bypassed that precedence model and let `None` in memory erase valid values that had already been resolved from file or env.
- At the persistence boundary, only accepting the new nested provider shape broke older saved config files even though the logical data model (`PartialConfig`) still had the same fields.

By accepting both on read and writing only the new canonical shape, the config file boundary becomes backward-compatible without preserving two output formats forever.

## Prevention

- When changing config serialization shape, add compatibility tests for both the old and new on-disk forms before removing old parsing paths.
- Do not replay non-secret config overlays after `config::Config::builder()` has already merged file/env sources. Encode precedence into source order instead.
- Add regression tests that prove `env > partial > file` for provider overrides, not just secrets.
- For setup-safe provider-only loading, keep `[llm]` parsing out of the path, but preserve the same provider precedence rules as full runtime loading.

## Related Issues

- `docs/solutions/best-practices/config-test-isolation-inline-toml-2026-04-11.md`
- `docs/solutions/logic-errors/cli-runtime-config-parity-and-setup-health-check-2026-04-15.md`
