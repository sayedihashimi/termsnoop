use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "termsnoop",
    version,
    about = "Terminal capture for AI CLI integration"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start a captured shell session
    Start {
        /// Human-friendly session name
        #[arg(long)]
        name: Option<String>,
        /// Shell to spawn (default: pwsh on Windows, $SHELL on Unix)
        #[arg(long)]
        shell: Option<String>,
    },
    /// List active and recent sessions
    List,
    /// Read output from a session
    Read {
        /// Session ID (latest session if omitted)
        session_id: Option<String>,
        /// Read last N commands' output
        #[arg(long)]
        last_commands: Option<usize>,
        /// Read last N lines
        #[arg(long)]
        tail: Option<usize>,
        /// Output as structured JSON
        #[arg(long)]
        json: bool,
    },
    /// Remove old session data
    Clean {
        /// Delete sessions older than N days
        #[arg(long, default_value = "7")]
        days: u64,
    },
    /// Run as MCP server over stdio
    #[command(name = "mcp-server")]
    McpServer,
}
