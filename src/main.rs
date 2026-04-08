mod boundary;
mod cli;
mod config;
mod mcp;
mod pty_proxy;
mod session;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::List) => session::list_sessions()?,
        Some(Commands::Read {
            session_id,
            last_commands,
            tail,
            json,
        }) => session::read_session(session_id, last_commands, tail, json)?,
        Some(Commands::Clean { days }) => session::clean_sessions(days)?,
        Some(Commands::McpServer) => mcp::run_server().await?,
        Some(Commands::Start { name, shell, debug }) => pty_proxy::start_session(name, shell, debug)?,
        None => pty_proxy::start_session(None, None, false)?,
    }

    Ok(())
}
