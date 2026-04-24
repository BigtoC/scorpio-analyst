# Implementation Gap Analysis: Integrating GitHub Copilot into scorpio-analyst

This document outlines the required implementation steps to integrate GitHub Copilot into `scorpio-analyst` using the `cc-switch` methodology.

To bridge the gap between `scorpio-analyst`'s existing LLM architecture (which likely expects standard REST APIs) and GitHub Copilot's proprietary authentication flow, you will need to implement four primary components in Rust.

## 1. Authentication & Token Lifecycle (The Biggest Gap)
Standard providers (like OpenAI or Anthropic) use a static API key. Copilot uses a dynamic, short-lived token system that `scorpio-analyst` currently does not support. 
* **Device Flow Authentication:** You must implement the GitHub OAuth Device Code flow to allow the user to authenticate and generate a `ghu_` (GitHub User) token.
* **Token Exchanger:** You need a background service or a lazy-loading mechanism that takes the `ghu_` token and requests a Copilot Access Token (`tid_`) from `https://api.github.com/copilot_internal/v2/token`.
* **Token Refresh Logic:** Because the Copilot token expires (usually after 30 minutes), your implementation must cache the token and automatically request a new one upon expiration before executing the next analysis API call.

## 2. Header Forgery & Client Configuration
Copilot's API backend (`api.githubcopilot.com`) is essentially an OpenAI-compatible endpoint, but it actively rejects requests that do not look like they are coming from an official IDE plugin.
* **Custom Headers:** Your new provider in `scorpio-analyst` must inject specific headers into every HTTP call. You will need to hardcode or mock headers such as:
    * `Authorization: Bearer <the_dynamic_copilot_token>`
    * `Editor-Version: vscode/1.85.0` (or similar)
    * `Editor-Plugin-Version: copilot-chat/0.11.1`
    * `User-Agent: GitHubCopilotChat/0.11.1`
    * `Vscode-Sessionid` and `Vscode-Machineid` (often required to bypass telemetry checks; can be randomly generated UUIDs).

## 3. Provider Trait Implementation (`src/providers/`)
You will need to create a new Rust module (e.g., `copilot.rs`) that adheres to `scorpio-analyst`'s internal provider trait (often a trait defining an `ask` or `generate` function).
* **Endpoint Mapping:** Map the target URL to `https://api.githubcopilot.com/chat/completions`.
* **Payload Adaptation:** Since Copilot uses the standard OpenAI payload format (Messages array, System prompt, Temperature), you can likely reuse `scorpio-analyst`'s existing OpenAI payload serialization logic.
* **Model Selection:** Copilot silently routes requests based on the model name. You will need to restrict the available models to `gpt-4`, `gpt-4-turbo`, or `gpt-3.5-turbo`, as custom or unsupported model names will result in a 400 Bad Request.

## 4. Configuration and State Management
`scorpio-analyst` will need a way to store the user's GitHub credentials persistently so they don't have to log in via OAuth every time they run an analysis.
* **Secret Storage:** Add a configuration block (likely in a `config.toml`, `.env`, or a local SQLite/JSON state file) to securely store the `ghu_` token.
