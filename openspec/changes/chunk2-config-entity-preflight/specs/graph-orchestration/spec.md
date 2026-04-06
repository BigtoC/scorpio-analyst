# `graph-orchestration` Capability

## MODIFIED Requirements

### Requirement: Graph-Flow Pipeline Construction

The system MUST construct a graph-flow directed graph whose first task is `PreflightTask`.

The pipeline topology MUST begin with:

- `PreflightTask("preflight")`
- edge `preflight -> analyst_fanout`

After preflight completes successfully, the existing analyst fan-out, debate, trading, risk, and fund-manager phases
continue unchanged.

The graph start task MUST be `preflight`, not `analyst_fanout`.

#### Scenario: Graph Builds With Preflight Start Node

- **WHEN** `GraphBuilder` constructs the trading pipeline after this change
- **THEN** the graph start task is `preflight`, and there is a direct edge from `preflight` to `analyst_fanout`

#### Scenario: Preflight Failure Stops Execution Before Analysts

- **WHEN** `PreflightTask` returns an error because symbol resolution fails or required preflight context cannot be written
- **THEN** pipeline execution halts before any analyst child task is dispatched
