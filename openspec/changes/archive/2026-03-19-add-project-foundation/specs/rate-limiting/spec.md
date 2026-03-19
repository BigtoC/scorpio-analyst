# `rate-limiting` Capability

## ADDED Requirements

### Requirement: Asynchronous Traffic Controlling System

The foundation MUST distribute provider-scoped rate controllers utilizing the `governor` crate. A
`governor::DefaultDirectRateLimiter` instance MUST be shareable through `Arc` so concurrent tasks coordinate access
without exceeding shared provider limits.

#### Scenario: Coordinating Inferences

When spawning 4 independent analyst tasks that all depend on the same upstream provider, the shared limiter enforces
micro-yields before outbound requests proceed, preventing aggregate traffic bursts from violating provider limits.

### Requirement: Configurable Per-Provider Traffic Handling

Rate-control configuration MUST be passed through typed configuration dependencies dictating explicit throttle values on
a per-provider basis. The foundation MUST provide a Finnhub default of 30 requests per second while allowing overrides
for other providers and environments.

#### Scenario: Aggressive API Scraping Operations

An agent begins fetching numerous historical timeline snapshots. Instead of immediately issuing 50 instant calls that
would cause an automated IP block, the shared limiter applies provider-specific quotas and spaces those requests within
the configured rate budget.

### Requirement: Dependency Injection For Limiters

The rate-limiting foundation MUST expose dependency-injection-friendly limiter handles so downstream data clients and
agent tasks can receive the correct provider limiter without constructing their own independent throttling state.

#### Scenario: Wiring A Data Client

When a downstream Finnhub client is instantiated, it receives the shared Finnhub limiter through its constructor and
awaits limiter readiness before making each outbound request.
