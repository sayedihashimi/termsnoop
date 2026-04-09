use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const SESSION_ID_LEN: usize = 7;
const ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub pid: u32,
    pub shell: String,
    pub cwd: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CommandEntry {
    pub index: usize,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub output: String,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

pub fn sessions_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot determine home directory")?;
    let dir = home.join(".termsnoop").join("sessions");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn session_dir(id: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(id))
}

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

pub fn generate_id() -> String {
    let mut rng = rand::thread_rng();
    (0..SESSION_ID_LEN)
        .map(|_| {
            let idx = rng.gen_range(0..ID_ALPHABET.len());
            ID_ALPHABET[idx] as char
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Create / update
// ---------------------------------------------------------------------------

pub fn create_session(name: Option<String>, shell: &str) -> Result<SessionMeta> {
    let id = generate_id();
    let dir = session_dir(&id)?;
    std::fs::create_dir_all(&dir)?;

    let meta = SessionMeta {
        id,
        name,
        pid: std::process::id(),
        shell: shell.to_string(),
        cwd: std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        started_at: Utc::now(),
        ended_at: None,
        status: "active".into(),
    };

    let path = dir.join("meta.json");
    std::fs::write(&path, serde_json::to_string_pretty(&meta)?)?;
    Ok(meta)
}

pub fn update_session_status(id: &str, status: &str) -> Result<()> {
    let path = session_dir(id)?.join("meta.json");
    let text = std::fs::read_to_string(&path)?;
    let mut meta: SessionMeta = serde_json::from_str(&text)?;
    meta.status = status.into();
    if status == "exited" {
        meta.ended_at = Some(Utc::now());
    }
    std::fs::write(&path, serde_json::to_string_pretty(&meta)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

pub fn list_sessions() -> Result<()> {
    let mut sessions = load_all_sessions()?;
    sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    if sessions.is_empty() {
        println!("No sessions found.");
        return Ok(());
    }

    println!(
        "{:<10} {:<15} {:<10} {:<8} {}",
        "ID", "Name", "Status", "Shell", "Started"
    );
    println!("{}", "-".repeat(70));
    for s in &sessions {
        println!(
            "{:<10} {:<15} {:<10} {:<8} {}",
            s.id,
            s.name.as_deref().unwrap_or("-"),
            s.status,
            s.shell,
            s.started_at.format("%Y-%m-%d %H:%M:%S")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Read
// ---------------------------------------------------------------------------

pub fn read_session(
    session_id: Option<String>,
    last_commands: Option<usize>,
    tail: Option<usize>,
    json: bool,
) -> Result<()> {
    let id = match session_id {
        Some(id) => id,
        None => find_latest_session()?,
    };

    let dir = session_dir(&id)?;
    anyhow::ensure!(dir.exists(), "session '{}' not found", id);

    if let Some(n) = last_commands {
        return read_commands(&dir, n, json);
    }

    read_output_log(&dir, &id, tail, json)
}

fn read_commands(dir: &PathBuf, n: usize, json: bool) -> Result<()> {
    let path = dir.join("commands.jsonl");
    anyhow::ensure!(
        path.exists(),
        "No structured command data. Command boundary detection may not be active.\n\
         Use --tail instead to read raw output."
    );

    let content = std::fs::read_to_string(&path)?;
    let entries: Vec<CommandEntry> = content
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let start = entries.len().saturating_sub(n);
    let slice = &entries[start..];

    if json {
        println!("{}", serde_json::to_string_pretty(slice)?);
    } else {
        for e in slice {
            println!("$ {}", e.command);
            if !e.output.is_empty() {
                println!("{}", e.output);
            }
            println!();
        }
    }
    Ok(())
}

fn read_output_log(dir: &PathBuf, id: &str, tail: Option<usize>, json: bool) -> Result<()> {
    let path = dir.join("output.log");
    anyhow::ensure!(path.exists(), "No output log for session '{}'", id);

    let raw = std::fs::read(&path)?;
    let stripped = strip_ansi_escapes::strip(&raw);
    let text = String::from_utf8_lossy(&stripped);

    let output = if let Some(n) = tail {
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    } else {
        text.into_owned()
    };

    if json {
        println!(
            "{}",
            serde_json::json!({
                "session_id": id,
                "lines": output.lines().count(),
                "output": output,
            })
        );
    } else {
        print!("{}", output);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Clean
// ---------------------------------------------------------------------------

pub fn clean_sessions(days: u64) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(days as i64);
    let mut cleaned = 0u32;

    for meta in load_all_sessions()? {
        if meta.started_at < cutoff {
            let dir = session_dir(&meta.id)?;
            if dir.exists() {
                std::fs::remove_dir_all(&dir)?;
                cleaned += 1;
            }
        }
    }

    println!("Cleaned {} session(s) older than {} days.", cleaned, days);
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_all_sessions() -> Result<Vec<SessionMeta>> {
    let dir = sessions_dir()?;
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
            if let Ok(meta) = serde_json::from_str::<SessionMeta>(&text) {
                out.push(meta);
            }
        }
    }

    Ok(out)
}

fn find_latest_session() -> Result<String> {
    let mut sessions = load_all_sessions()?;
    sessions.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    sessions
        .first()
        .map(|s| s.id.clone())
        .ok_or_else(|| anyhow::anyhow!("No sessions found. Start one with: termsnoop start"))
}
