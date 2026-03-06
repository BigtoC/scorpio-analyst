# Scorpio-Analyst: A Rust-Native Multi-Agent Financial Trading System

Scorpio-Analyst is a Rust-native reimplementation of the [TradingAgents framework](https://github.com/TauricResearch/TradingAgents), inspired by the paper [_TradingAgents: Multi-Agents LLM Financial Trading Framework_](https://arxiv.org/pdf/2412.20138). It simulates a sophisticated trading firm by employing a society of specialized AI agents that collaborate to make autonomous, explainable financial trading decisions.

The project's primary goal is to overcome the limitations of traditional algorithmic trading and monolithic AI systems by leveraging a structured, multi-agent approach. This allows for the integration of qualitative data, enhances explainability, and achieves superior risk-adjusted returns.

![Project Infographic](docs/images/infographic.png)

## Conceptual Foundation

The system is built on two core principles from the original TradingAgents paradigm:

1.  **Organizational Modeling**: Instead of a single AI trying to do everything, the system decomposes the trading lifecycle into highly specialized roles (Analysts, Researchers, a Trader, Risk Managers, and a Fund Manager). This mirrors the structure of a real-world trading firm, preventing cognitive overload and improving decision quality.

2.  **Structured Communication**: To combat the "telephone effect" where data degrades in unstructured conversations, agents communicate through strictly-typed, structured data reports. This ensures that critical information is passed with perfect fidelity throughout the execution pipeline.

## High-Level Execution Graph

The system operates as a stateful workflow, orchestrating the collaboration between different agent teams in a 5-phase execution pipeline.

```mermaid
graph TD
%% Core State
    Start((Trade Trigger)) --> FanOutAnalysts

    subgraph Analyst_Team
        FanOutAnalysts
        FanOutAnalysts --> Fund[Fundamental Analyst]
        FanOutAnalysts --> Sent[Sentiment Analysts]
        FanOutAnalysts --> News[News Analyst]
        FanOutAnalysts --> Tech[Technical Analysts]
    end

    Fund --> SyncAnalysts
    Sent --> SyncAnalysts
    News --> SyncAnalysts
    Tech --> SyncAnalysts

    subgraph Researcher_Team
        SyncAnalysts --> Bull
        SyncAnalysts --> Bear
        Bear --> Moderator{Debate Moderator}
        Bull --> Moderator{Debate Moderator}
        Moderator -- Max Rounds Not Reached --> Moderator
    end

    subgraph Synthesis_Execution
        Moderator -- Max Rounds Reached --> Trader
    end

    subgraph Risk_Team [Phase 4: Risk Discussion]
        Trader --> RiskSeeking
        Trader --> RiskConservative
        Trader --> RiskNeutral
        RiskSeeking --> RiskModerator{Risk Moderator}
        RiskConservative --> RiskModerator
        RiskNeutral --> RiskModerator
        RiskModerator -- Max Rounds Not Reached --> RiskModerator
    end

    subgraph Final_Decision [Phase 5: Managerial Arbitration]
        RiskModerator -- Max Rounds Reached --> Manager{Fund Manager}
        Manager -- Approve --> Execute((Execute Trade))
        Manager -- Reject --> Abort((Terminate))
    end
```

## User Interaction

Scorpio-Analyst is designed with a phased user interface approach to provide both power and ease of use:

*   **Phase 1 (MVP)**: A comprehensive Command-Line Interface (CLI) built with `clap`, supporting both structured subcommands and natural language queries.
*   **Phase 2**: An interactive Terminal User Interface (TUI) for a rich, conversational experience.
*   **Phase 3**: A high-performance, GPU-accelerated native desktop application.

## Project Status

This project is in the early stages of development. The architecture and core components are being actively built.

For a deep dive into the system's architecture, agent roles, and technical specifications, please see the [**Product Requirements Document (PRD.md)**](PRD.md).

Contributions are welcome!
