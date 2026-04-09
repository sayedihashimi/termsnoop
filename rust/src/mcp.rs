use anyhow::Result;
use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt,
    transport::stdio,
};

use crate::session;

// ---------------------------------------------------------------------------
// Request schemas
// ---------------------------------------------------------------------------

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReadTerminalRequest {
    #[schemars(description = "Session ID to read from. If omitted, reads the latest session.")]
    pub session_id: Option<String>,

    #[schemars(description = "Number of recent commands to return (requires command tracking).")]
    pub last_commands: Option<usize>,

    #[schemars(description = "Number of lines to return from the end of the output log.")]
    pub tail_lines: Option<usize>,
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct TermsnoopServer {
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl TermsnoopServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(description = "Read recent terminal output from a termsnoop session. \
        Returns command+output pairs when command tracking is active, \
        or raw output lines when using tail_lines.")]
    fn read_terminal_output(
        &self,
        Parameters(req): Parameters<ReadTerminalRequest>,
    ) -> String {
        let id = match &req.session_id {
            Some(id) => id.clone(),
            None => match find_latest() {
                Ok(id) => id,
                Err(e) => return format!("Error: {}", e),
            },
        };

        let dir = match session::session_dir(&id) {
            Ok(d) => d,
            Err(e) => return format!("Error: {}", e),
        };

        if !dir.exists() {
            return format!("Error: session '{}' not found", id);
        }

        // --last-commands mode
        if let Some(n) = req.last_commands {
            let path = dir.join("commands.jsonl");
            if !path.exists() {
                return "No structured command data available. \
                        Try using tail_lines instead."
                    .to_string();
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => return format!("Error reading commands: {}", e),
            };
            let entries: Vec<session::CommandEntry> = content
                .lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            let start = entries.len().saturating_sub(n);
            let slice = &entries[start..];
            return serde_json::to_string_pretty(slice).unwrap_or_default();
        }

        // --tail mode (or full output)
        let log_path = dir.join("output.log");
        if !log_path.exists() {
            return format!("No output log for session '{}'", id);
        }
        let raw = match std::fs::read(&log_path) {
            Ok(r) => r,
            Err(e) => return format!("Error reading log: {}", e),
        };
        let stripped = strip_ansi_escapes::strip(&raw);
        let text = String::from_utf8_lossy(&stripped);

        if let Some(n) = req.tail_lines {
            let lines: Vec<&str> = text.lines().collect();
            let start = lines.len().saturating_sub(n);
            lines[start..].join("\n")
        } else {
            text.into_owned()
        }
    }

    #[tool(description = "List active and recent termsnoop terminal sessions.")]
    fn list_terminal_sessions(&self) -> String {
        let sessions = match load_sessions() {
            Ok(s) => s,
            Err(e) => return format!("Error: {}", e),
        };

        if sessions.is_empty() {
            return "No sessions found.".to_string();
        }

        serde_json::to_string_pretty(&sessions).unwrap_or_default()
    }
}

#[tool_handler]
impl ServerHandler for TermsnoopServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "termsnoop lets you read terminal output from other terminal sessions. \
                 Use list_terminal_sessions to discover sessions, then \
                 read_terminal_output to get their output."
                    .to_string(),
            )
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run_server() -> Result<()> {
    let server = TermsnoopServer::new();
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers (non-pub wrappers around session module)
// ---------------------------------------------------------------------------

fn find_latest() -> Result<String> {
    let mut sessions = load_sessions()?;
    sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    sessions
        .first()
        .map(|s| s.id.clone())
        .ok_or_else(|| anyhow::anyhow!("No sessions found"))
}

fn load_sessions() -> Result<Vec<session::SessionMeta>> {
    let dir = session::sessions_dir()?;
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let meta_path = entry.path().join("meta.json");
        if let Ok(text) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<session::SessionMeta>(&text) {
                out.push(meta);
            }
        }
    }
    out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(out)
}
