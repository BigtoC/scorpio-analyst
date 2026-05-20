//! `scorpio-server` binary entry point.
//!
//! Delegates all CLI surface (subcommands, environment selection, port/binding
//! overrides) to Loco's built-in CLI. The most common invocations are:
//!
//! ```text
//! scorpio-server start                       # bind per config/<env>.yaml
//! scorpio-server start -b 0.0.0.0 -p 8088    # override binding/port
//! scorpio-server routes                      # print every mounted route
//! scorpio-server doctor                      # validate configuration
//! ```
//!
//! Environment selection follows Loco: set `LOCO_ENV=production` (or pass
//! `--environment production`) to load `config/production.yaml`.

use loco_rs::cli;
use scorpio_server::app::App;

#[tokio::main]
async fn main() -> loco_rs::Result<()> {
    cli::main::<App>().await
}
