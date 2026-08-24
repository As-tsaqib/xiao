use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::telegram::TelegramScope;

#[derive(Debug)]
pub struct Storage {
    conn: Mutex<Connection>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRecord {
    pub id: String,
    pub owner_principal: String,
    pub name: String,
    pub provider: String,
    pub account_id: Option<String>,
    pub model: String,
    pub message_count: i64,
    pub archived: bool,
    pub is_side: bool,
    pub parent_id: Option<String>,
    pub yolo_mode: bool,
    pub created_at: String,
    pub last_active_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredMessageRecord {
    pub id: i64,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSummaryRecord {
    pub session_id: String,
    pub owner_principal: String,
    pub summary: String,
    pub covered_through_message_id: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub email: Option<String>,
    pub status: String,
    pub access_expires_at: Option<String>,
    pub metadata_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramInboxRecord {
    pub update_id: i64,
    pub payload_json: String,
    pub status: String,
    pub attempts: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentRunRecord {
    pub id: String,
    pub owner_principal: String,
    pub session_id: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub goal: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolRunRecord {
    pub id: String,
    pub agent_run_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub arguments_json: String,
    pub risk: String,
    pub approval_mode: Option<String>,
    pub policy_original: Option<String>,
    pub status: String,
    pub output: Option<String>,
    pub error: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRecord {
    pub id: String,
    pub owner_principal: String,
    pub session_id: String,
    pub agent_run_id: String,
    pub tool_call_id: String,
    pub capability: String,
    pub tool_name: String,
    pub arguments_hash: String,
    pub risk: String,
    pub summary: String,
    pub status: String,
    pub approval_mode: Option<String>,
    pub requested_at: String,
    pub decided_at: Option<String>,
    pub expires_at: String,
    pub consumed_at: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct ApprovalBinding<'a> {
    pub owner_id: &'a str,
    pub session_id: &'a str,
    pub agent_run_id: &'a str,
    pub tool_call_id: &'a str,
    pub tool_name: &'a str,
    pub arguments_hash: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub struct ApprovalRequest<'a> {
    pub binding: ApprovalBinding<'a>,
    pub capability: &'a str,
    pub risk: &'a str,
    pub summary: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DependencyInstallRecord {
    pub id: String,
    pub agent_run_id: Option<String>,
    pub binary: String,
    pub package: String,
    pub package_manager: String,
    pub source: String,
    pub validated: bool,
    pub requested_capability: Option<String>,
    pub status: String,
    pub evidence: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct DependencyInstallStart<'a> {
    pub agent_run_id: Option<&'a str>,
    pub binary: &'a str,
    pub package: &'a str,
    pub package_manager: &'a str,
    pub source: &'a str,
    pub validated: bool,
    pub requested_capability: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderCapabilityRecord {
    pub provider: String,
    pub model: String,
    pub tool_protocol: String,
    pub native_tool_calls: bool,
    pub structured_output: bool,
    pub continuation: bool,
    pub probed_at: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfileRecord {
    pub profile_id: String,
    pub owner_id: String,
    pub alias: String,
    pub endpoint: String,
    pub protocol: String,
    pub credential_ref: Option<String>,
    pub safe_headers_json: String,
    pub secret_headers_ref: Option<String>,
    pub enabled: bool,
    pub reachability: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_probe_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderProfileModelRecord {
    pub profile_id: String,
    pub model_id: String,
    pub text_capable: bool,
    pub vision_capable: bool,
    pub file_input_capable: bool,
    pub native_tools: bool,
    pub structured_output: bool,
    pub continuation: bool,
    /// Tri-state probe results. Boolean fields above are runtime-compatible
    /// "supported" projections and must never be used to represent Unknown.
    pub native_tools_state: String,
    pub structured_output_state: String,
    pub continuation_state: String,
    pub vision_state: String,
    pub file_input_state: String,
    pub model_discovery: bool,
    pub tool_protocol: String,
    pub evidence: String,
    pub probed_at: String,
}

#[derive(Debug, Clone)]
pub struct ProviderProfileInput {
    pub profile_id: Option<String>,
    pub owner_id: String,
    pub alias: String,
    pub endpoint: String,
    pub protocol: String,
    pub credential_ref: Option<String>,
    pub safe_headers_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerMigrationResult {
    pub owner_id: String,
    pub migrated_legacy_principals: usize,
    pub requires_file_reconcile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentRecord {
    pub attachment_id: String,
    pub owner_id: String,
    pub session_id: String,
    pub telegram_file_id: Option<String>,
    pub telegram_unique_id: Option<String>,
    pub original_name: String,
    pub declared_mime: Option<String>,
    pub detected_mime: String,
    pub kind: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub local_path: String,
    pub processing_status: String,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct NewAttachmentRecord<'a> {
    pub attachment_id: &'a str,
    pub owner_id: &'a str,
    pub session_id: &'a str,
    pub telegram_file_id: Option<&'a str>,
    pub telegram_unique_id: Option<&'a str>,
    pub original_name: &'a str,
    pub declared_mime: Option<&'a str>,
    pub detected_mime: &'a str,
    pub kind: &'a str,
    pub size_bytes: u64,
    pub sha256: &'a str,
    pub local_path: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttachmentChunkRecord {
    pub attachment_id: String,
    pub chunk_no: usize,
    pub page_no: Option<usize>,
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuditEventRecord {
    pub id: i64,
    pub principal: String,
    pub action: String,
    pub detail: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManagerCounts {
    pub sessions: usize,
    pub messages: usize,
    pub agent_runs: usize,
    pub running_runs: usize,
    pub blocked_runs: usize,
    pub memories: usize,
    pub skills: usize,
    pub attachments: usize,
    pub pending_approvals: usize,
}

impl Storage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn =
            Connection::open(path).with_context(|| format!("open sqlite {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let s = Self {
            conn: Mutex::new(conn),
        };
        s.migrate()?;
        s.recover_interrupted_runs()?;
        Ok(s)
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let s = Self {
            conn: Mutex::new(conn),
        };
        s.migrate()?;
        s.recover_interrupted_runs()?;
        Ok(s)
    }

    pub fn migrate(&self) -> Result<()> {
        self.with_conn(|conn| {
        conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS schema_migrations(version INTEGER PRIMARY KEY);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(1);
        CREATE TABLE IF NOT EXISTS sessions(
          id TEXT PRIMARY KEY, name TEXT NOT NULL, provider TEXT NOT NULL DEFAULT 'custom',
          account_id TEXT, model TEXT NOT NULL DEFAULT 'default', archived INTEGER NOT NULL DEFAULT 0,
          is_side INTEGER NOT NULL DEFAULT 0, parent_id TEXT,
          created_at TEXT NOT NULL, last_active_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS messages(
          id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
          role TEXT NOT NULL, content TEXT NOT NULL, created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS frontend_state(
          principal TEXT PRIMARY KEY, active_main_session_id TEXT NOT NULL,
          side_session_id TEXT, mode TEXT NOT NULL DEFAULT 'main'
        );
        CREATE TABLE IF NOT EXISTS provider_accounts(
          id TEXT PRIMARY KEY, provider TEXT NOT NULL, label TEXT NOT NULL, email TEXT,
          status TEXT NOT NULL, access_expires_at TEXT, metadata_json TEXT NOT NULL DEFAULT '{}',
          created_at TEXT NOT NULL, updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_provider_accounts_provider ON provider_accounts(provider);
        CREATE TABLE IF NOT EXISTS kv_settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS telegram_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS audit_events(
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          principal TEXT NOT NULL,
          action TEXT NOT NULL,
          detail TEXT NOT NULL DEFAULT '',
          created_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(2);
        CREATE TABLE IF NOT EXISTS access_principals(
          principal TEXT PRIMARY KEY,
          role TEXT NOT NULL DEFAULT 'user',
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS telegram_chats(
          chat_id INTEGER PRIMARY KEY,
          chat_type TEXT NOT NULL,
          title TEXT,
          last_seen_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS provider_native_sessions(
          session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
          provider TEXT NOT NULL,
          native_session_id TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          PRIMARY KEY(session_id, provider)
        );
        CREATE TABLE IF NOT EXISTS usage_events(
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
          provider TEXT NOT NULL,
          model TEXT NOT NULL,
          input_units INTEGER,
          output_units INTEGER,
          metadata_json TEXT NOT NULL DEFAULT '{}',
          created_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(3);
        CREATE TABLE IF NOT EXISTS telegram_inbox(
          update_id INTEGER PRIMARY KEY,
          payload_json TEXT NOT NULL,
          status TEXT NOT NULL DEFAULT 'pending',
          attempts INTEGER NOT NULL DEFAULT 0,
          received_at TEXT NOT NULL,
          processed_at TEXT,
          last_error TEXT
        );
        "#)?;
        let has_owner = {
            let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
            let names = stmt.query_map([], |r| r.get::<_, String>(1))?.collect::<rusqlite::Result<Vec<_>>>()?;
            names.iter().any(|name| name == "owner_principal")
        };
        if !has_owner {
            conn.execute("ALTER TABLE sessions ADD COLUMN owner_principal TEXT NOT NULL DEFAULT 'legacy:unassigned'", [])?;
        }
        // Legacy v0.1.0 sessions were global. Assign a legacy main session to at most
        // one historical principal; all other principals will get a fresh session.
        conn.execute_batch(r#"
        UPDATE sessions
           SET owner_principal=COALESCE(
             (SELECT principal FROM frontend_state f WHERE f.active_main_session_id=sessions.id ORDER BY principal LIMIT 1),
             'legacy:unassigned')
         WHERE is_side=0 AND owner_principal='legacy:unassigned';
        UPDATE sessions AS child
           SET owner_principal=COALESCE((SELECT parent.owner_principal FROM sessions parent WHERE parent.id=child.parent_id), child.owner_principal)
         WHERE child.is_side=1;
        CREATE INDEX IF NOT EXISTS idx_sessions_owner_main ON sessions(owner_principal,is_side,archived,last_active_at);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(4),(5);
        "#)?;
        conn.execute_batch(r#"
        CREATE TABLE IF NOT EXISTS agent_runs(
          id TEXT PRIMARY KEY,
          owner_principal TEXT NOT NULL,
          session_id TEXT NOT NULL REFERENCES sessions(id),
          provider TEXT NOT NULL,
          model TEXT NOT NULL,
          status TEXT NOT NULL,
          goal TEXT,
          started_at TEXT NOT NULL,
          finished_at TEXT,
          error TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_runs_owner_started
          ON agent_runs(owner_principal,started_at DESC);
        CREATE INDEX IF NOT EXISTS idx_agent_runs_session_started
          ON agent_runs(session_id,started_at DESC);
        CREATE TABLE IF NOT EXISTS tool_runs(
          id TEXT PRIMARY KEY,
          agent_run_id TEXT NOT NULL REFERENCES agent_runs(id),
          call_id TEXT NOT NULL,
          tool_name TEXT NOT NULL,
          arguments_json TEXT NOT NULL,
          risk TEXT NOT NULL,
          status TEXT NOT NULL,
          output TEXT,
          error TEXT,
          started_at TEXT,
          finished_at TEXT,
          UNIQUE(agent_run_id,call_id)
        );
        CREATE INDEX IF NOT EXISTS idx_tool_runs_agent
          ON tool_runs(agent_run_id,started_at);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(6);
        CREATE TABLE IF NOT EXISTS memories(
          id TEXT PRIMARY KEY,
          owner_principal TEXT NOT NULL,
          scope TEXT NOT NULL CHECK(scope IN ('user','agent')),
          category TEXT NOT NULL,
          key TEXT NOT NULL,
          value TEXT NOT NULL,
          confidence REAL NOT NULL DEFAULT 1.0,
          source_kind TEXT NOT NULL,
          source_session_id TEXT,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          UNIQUE(owner_principal,scope,category,key)
        );
        CREATE INDEX IF NOT EXISTS idx_memories_owner_scope_category
          ON memories(owner_principal,scope,category,updated_at DESC);
        CREATE TABLE IF NOT EXISTS memory_history(
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          memory_id TEXT,
          owner_principal TEXT NOT NULL,
          scope TEXT NOT NULL,
          category TEXT NOT NULL,
          key TEXT NOT NULL,
          action TEXT NOT NULL,
          old_value TEXT,
          new_value TEXT,
          source_session_id TEXT,
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_memory_history_owner_memory
          ON memory_history(owner_principal,memory_id,created_at DESC);
        CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
          owner_principal UNINDEXED,
          scope UNINDEXED,
          category,
          key,
          value
        );
        CREATE TRIGGER IF NOT EXISTS memories_fts_insert AFTER INSERT ON memories BEGIN
          INSERT INTO memories_fts(rowid,owner_principal,scope,category,key,value)
          VALUES(new.rowid,new.owner_principal,new.scope,new.category,new.key,new.value);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_fts_update AFTER UPDATE ON memories BEGIN
          DELETE FROM memories_fts WHERE rowid=old.rowid;
          INSERT INTO memories_fts(rowid,owner_principal,scope,category,key,value)
          VALUES(new.rowid,new.owner_principal,new.scope,new.category,new.key,new.value);
        END;
        CREATE TRIGGER IF NOT EXISTS memories_fts_delete AFTER DELETE ON memories BEGIN
          DELETE FROM memories_fts WHERE rowid=old.rowid;
        END;
        INSERT INTO memories_fts(rowid,owner_principal,scope,category,key,value)
          SELECT m.rowid,m.owner_principal,m.scope,m.category,m.key,m.value
          FROM memories m
          WHERE NOT EXISTS(SELECT 1 FROM memories_fts f WHERE f.rowid=m.rowid);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(7);
        CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(content);
        CREATE TRIGGER IF NOT EXISTS messages_fts_insert AFTER INSERT ON messages BEGIN
          INSERT INTO messages_fts(rowid,content) VALUES(new.id,new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS messages_fts_update AFTER UPDATE ON messages BEGIN
          DELETE FROM messages_fts WHERE rowid=old.id;
          INSERT INTO messages_fts(rowid,content) VALUES(new.id,new.content);
        END;
        CREATE TRIGGER IF NOT EXISTS messages_fts_delete AFTER DELETE ON messages BEGIN
          DELETE FROM messages_fts WHERE rowid=old.id;
        END;
        INSERT INTO messages_fts(rowid,content)
          SELECT m.id,m.content FROM messages m
          WHERE NOT EXISTS(SELECT 1 FROM messages_fts f WHERE f.rowid=m.id);
        CREATE TABLE IF NOT EXISTS session_summaries(
          session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
          owner_principal TEXT NOT NULL,
          summary TEXT NOT NULL,
          covered_through_message_id INTEGER NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_session_summaries_owner
          ON session_summaries(owner_principal,updated_at DESC);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(8);
        CREATE TABLE IF NOT EXISTS skills(
          id TEXT PRIMARY KEY,
          owner_principal TEXT NOT NULL,
          name TEXT NOT NULL,
          summary TEXT NOT NULL,
          when_to_use TEXT NOT NULL,
          procedure TEXT NOT NULL,
          pitfalls TEXT NOT NULL DEFAULT '',
          verification TEXT NOT NULL DEFAULT '',
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          UNIQUE(owner_principal,name)
        );
        CREATE INDEX IF NOT EXISTS idx_skills_owner_updated
          ON skills(owner_principal,updated_at DESC);
        CREATE TABLE IF NOT EXISTS skill_history(
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          skill_id TEXT,
          owner_principal TEXT NOT NULL,
          action TEXT NOT NULL,
          old_content_json TEXT,
          new_content_json TEXT,
          source_session_id TEXT,
          created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_skill_history_owner_skill
          ON skill_history(owner_principal,skill_id,created_at DESC);
        CREATE VIRTUAL TABLE IF NOT EXISTS skills_fts USING fts5(
          owner_principal UNINDEXED,
          name,
          summary,
          when_to_use,
          procedure
        );
        CREATE TRIGGER IF NOT EXISTS skills_fts_insert AFTER INSERT ON skills BEGIN
          INSERT INTO skills_fts(rowid,owner_principal,name,summary,when_to_use,procedure)
          VALUES(new.rowid,new.owner_principal,new.name,new.summary,new.when_to_use,new.procedure);
        END;
        CREATE TRIGGER IF NOT EXISTS skills_fts_update AFTER UPDATE ON skills BEGIN
          DELETE FROM skills_fts WHERE rowid=old.rowid;
          INSERT INTO skills_fts(rowid,owner_principal,name,summary,when_to_use,procedure)
          VALUES(new.rowid,new.owner_principal,new.name,new.summary,new.when_to_use,new.procedure);
        END;
        CREATE TRIGGER IF NOT EXISTS skills_fts_delete AFTER DELETE ON skills BEGIN
          DELETE FROM skills_fts WHERE rowid=old.rowid;
        END;
        INSERT INTO skills_fts(rowid,owner_principal,name,summary,when_to_use,procedure)
          SELECT s.rowid,s.owner_principal,s.name,s.summary,s.when_to_use,s.procedure
          FROM skills s
          WHERE NOT EXISTS(SELECT 1 FROM skills_fts f WHERE f.rowid=s.rowid);
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(9);
        CREATE TABLE IF NOT EXISTS approvals(
          id TEXT PRIMARY KEY,
          owner_principal TEXT NOT NULL,
          capability TEXT NOT NULL,
          tool_name TEXT NOT NULL,
          arguments_hash TEXT NOT NULL,
          summary TEXT NOT NULL,
          status TEXT NOT NULL CHECK(status IN ('pending','approved','consumed','denied','expired')),
          requested_at TEXT NOT NULL,
          decided_at TEXT,
          expires_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_approvals_owner_status
          ON approvals(owner_principal,status,requested_at DESC);
        CREATE TABLE IF NOT EXISTS dependency_installs(
          id TEXT PRIMARY KEY,
          agent_run_id TEXT REFERENCES agent_runs(id) ON DELETE SET NULL,
          binary TEXT NOT NULL,
          package TEXT NOT NULL,
          package_manager TEXT NOT NULL,
          status TEXT NOT NULL CHECK(status IN ('installing','succeeded','failed','interrupted')),
          evidence TEXT,
          started_at TEXT NOT NULL,
          finished_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_dependency_installs_run
          ON dependency_installs(agent_run_id,started_at DESC);
        CREATE TABLE IF NOT EXISTS environment_probes(
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          snapshot_json TEXT NOT NULL,
          probed_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS workspace_file_index(
          path TEXT PRIMARY KEY,
          kind TEXT NOT NULL,
          content_hash TEXT NOT NULL,
          indexed_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS skill_file_index(
          path TEXT PRIMARY KEY,
          skill_name TEXT NOT NULL,
          content_hash TEXT NOT NULL,
          indexed_at TEXT NOT NULL
        );
        INSERT OR IGNORE INTO schema_migrations(version) VALUES(10);
        "#)?;
            ensure_column(
                conn,
                "sessions",
                "yolo_mode",
                "INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(conn, "tool_runs", "approval_mode", "TEXT")?;
            ensure_column(conn, "tool_runs", "policy_original", "TEXT")?;
            ensure_column(conn, "skills", "prerequisites", "TEXT NOT NULL DEFAULT ''")?;
            ensure_column(conn, "dependency_installs", "source", "TEXT NOT NULL DEFAULT 'known_mapping'")?;
            ensure_column(conn, "dependency_installs", "validated", "INTEGER NOT NULL DEFAULT 1")?;
            ensure_column(conn, "dependency_installs", "requested_capability", "TEXT")?;
            ensure_column(
                conn,
                "skills",
                "source_kind",
                "TEXT NOT NULL DEFAULT 'learned'",
            )?;
            ensure_column(conn, "skills", "enabled", "INTEGER NOT NULL DEFAULT 1")?;
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS telegram_session_scopes(
                  session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                  owner_principal TEXT NOT NULL,
                  chat_id INTEGER NOT NULL,
                  thread_id_key INTEGER NOT NULL DEFAULT 0,
                  is_side INTEGER NOT NULL DEFAULT 0,
                  created_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_telegram_session_scope
                  ON telegram_session_scopes(owner_principal,chat_id,thread_id_key,is_side);
                CREATE TABLE IF NOT EXISTS telegram_active_sessions(
                  owner_principal TEXT NOT NULL,
                  chat_id INTEGER NOT NULL,
                  thread_id_key INTEGER NOT NULL DEFAULT 0,
                  active_main_session_id TEXT NOT NULL REFERENCES sessions(id),
                  side_session_id TEXT REFERENCES sessions(id),
                  mode TEXT NOT NULL DEFAULT 'main' CHECK(mode IN ('main','side')),
                  updated_at TEXT NOT NULL,
                  PRIMARY KEY(owner_principal,chat_id,thread_id_key)
                );
                CREATE INDEX IF NOT EXISTS idx_telegram_active_session
                  ON telegram_active_sessions(active_main_session_id);
                CREATE TABLE IF NOT EXISTS provider_capabilities(
                  provider TEXT NOT NULL,
                  model TEXT NOT NULL,
                  tool_protocol TEXT NOT NULL,
                  native_tool_calls INTEGER NOT NULL DEFAULT 0,
                  structured_output INTEGER NOT NULL DEFAULT 0,
                  continuation INTEGER NOT NULL DEFAULT 0,
                  probed_at TEXT NOT NULL,
                  evidence TEXT NOT NULL DEFAULT '',
                  PRIMARY KEY(provider,model)
                );

                -- Pre-topic Telegram principals embed their chat before the owner id.
                -- Bind those sessions to the non-topic scope without rewriting any
                -- session/message/memory/skill ownership rows.
                INSERT OR IGNORE INTO telegram_session_scopes(
                  session_id,owner_principal,chat_id,thread_id_key,is_side,created_at
                )
                SELECT id,owner_principal,
                       CAST(substr(substr(owner_principal,10),1,instr(substr(owner_principal,10),':')-1) AS INTEGER),
                       0,is_side,created_at
                  FROM sessions
                 WHERE owner_principal LIKE 'telegram:%:%'
                   AND instr(substr(owner_principal,10),':') > 1;
                INSERT OR IGNORE INTO telegram_active_sessions(
                  owner_principal,chat_id,thread_id_key,active_main_session_id,side_session_id,mode,updated_at
                )
                SELECT f.principal,
                       CAST(substr(substr(f.principal,10),1,instr(substr(f.principal,10),':')-1) AS INTEGER),
                       0,f.active_main_session_id,f.side_session_id,f.mode,
                       strftime('%Y-%m-%dT%H:%M:%fZ','now')
                  FROM frontend_state f
                 WHERE f.principal LIKE 'telegram:%:%'
                   AND instr(substr(f.principal,10),':') > 1;
                INSERT OR IGNORE INTO schema_migrations(version) VALUES(11);
                "#,
            )?;
            conn.execute_batch(
                r#"
                UPDATE skills
                   SET source_kind='imported'
                 WHERE source_kind='learned'
                   AND id IN (
                     SELECT skill_id FROM skill_history
                      WHERE action='create' AND source_session_id IS NULL
                   );
                INSERT OR IGNORE INTO schema_migrations(version) VALUES(12);
                "#,
            )?;
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS owners(
                  owner_id TEXT PRIMARY KEY,
                  telegram_user_id INTEGER UNIQUE,
                  created_at TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS legacy_owner_principals(
                  legacy_principal TEXT PRIMARY KEY,
                  owner_id TEXT NOT NULL REFERENCES owners(owner_id),
                  migrated_at TEXT NOT NULL
                );
                INSERT OR IGNORE INTO schema_migrations(version) VALUES(13);
                "#,
            )?;
            ensure_column(conn, "approvals", "session_id", "TEXT NOT NULL DEFAULT ''")?;
            ensure_column(conn, "approvals", "agent_run_id", "TEXT NOT NULL DEFAULT ''")?;
            ensure_column(conn, "approvals", "tool_call_id", "TEXT NOT NULL DEFAULT ''")?;
            ensure_column(conn, "approvals", "risk", "TEXT NOT NULL DEFAULT 'unknown'")?;
            ensure_column(conn, "approvals", "approval_mode", "TEXT")?;
            ensure_column(conn, "approvals", "consumed_at", "TEXT")?;
            ensure_column(conn, "provider_accounts", "owner_id", "TEXT")?;
            conn.execute_batch(
                r#"
                UPDATE approvals
                   SET status='expired'
                 WHERE status IN ('pending','approved')
                   AND (session_id='' OR agent_run_id='' OR tool_call_id='');
                CREATE INDEX IF NOT EXISTS idx_approvals_exact_binding
                  ON approvals(owner_principal,session_id,agent_run_id,tool_call_id,tool_name,arguments_hash,status);
                CREATE TABLE IF NOT EXISTS provider_profiles(
                  profile_id TEXT PRIMARY KEY,
                  owner_id TEXT NOT NULL REFERENCES owners(owner_id),
                  provider_kind TEXT NOT NULL CHECK(provider_kind='custom'),
                  alias TEXT NOT NULL,
                  endpoint TEXT NOT NULL,
                  protocol TEXT NOT NULL,
                  credential_ref TEXT,
                  safe_headers_json TEXT NOT NULL DEFAULT '{}',
                  secret_headers_ref TEXT,
                  enabled INTEGER NOT NULL DEFAULT 1,
                  reachability TEXT NOT NULL DEFAULT 'unknown',
                  created_at TEXT NOT NULL,
                  updated_at TEXT NOT NULL,
                  last_probe_at TEXT,
                  UNIQUE(owner_id,alias)
                );
                CREATE INDEX IF NOT EXISTS idx_provider_profiles_owner
                  ON provider_profiles(owner_id,updated_at DESC);
                CREATE TABLE IF NOT EXISTS provider_profile_models(
                  profile_id TEXT NOT NULL REFERENCES provider_profiles(profile_id) ON DELETE CASCADE,
                  model_id TEXT NOT NULL,
                  text_capable INTEGER NOT NULL DEFAULT 1,
                  vision_capable INTEGER NOT NULL DEFAULT 0,
                  file_input_capable INTEGER NOT NULL DEFAULT 0,
                  native_tools INTEGER NOT NULL DEFAULT 0,
                  structured_output INTEGER NOT NULL DEFAULT 0,
                  continuation INTEGER NOT NULL DEFAULT 0,
                  model_discovery INTEGER NOT NULL DEFAULT 1,
                  tool_protocol TEXT NOT NULL DEFAULT 'chat_only',
                  evidence TEXT NOT NULL DEFAULT '',
                  probed_at TEXT NOT NULL,
                  PRIMARY KEY(profile_id,model_id)
                );
                INSERT OR IGNORE INTO schema_migrations(version) VALUES(14),(15);
                "#,
            )?;
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS attachments(
                  attachment_id TEXT PRIMARY KEY,
                  owner_id TEXT NOT NULL,
                  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                  telegram_file_id TEXT,
                  telegram_unique_id TEXT,
                  original_name TEXT NOT NULL,
                  declared_mime TEXT,
                  detected_mime TEXT NOT NULL,
                  kind TEXT NOT NULL CHECK(kind IN ('image','document')),
                  size_bytes INTEGER NOT NULL CHECK(size_bytes>=0),
                  sha256 TEXT NOT NULL,
                  local_path TEXT NOT NULL,
                  processing_status TEXT NOT NULL CHECK(processing_status IN ('downloaded','processing','ready','needs_ocr','rejected','failed')),
                  summary TEXT,
                  error TEXT,
                  created_at TEXT NOT NULL,
                  updated_at TEXT NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_attachments_session_created
                  ON attachments(owner_id,session_id,created_at DESC);
                CREATE INDEX IF NOT EXISTS idx_attachments_hash
                  ON attachments(owner_id,sha256);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_attachments_telegram_unique
                  ON attachments(owner_id,session_id,telegram_unique_id)
                  WHERE telegram_unique_id IS NOT NULL;
                CREATE TABLE IF NOT EXISTS attachment_chunks(
                  id INTEGER PRIMARY KEY AUTOINCREMENT,
                  attachment_id TEXT NOT NULL REFERENCES attachments(attachment_id) ON DELETE CASCADE,
                  chunk_no INTEGER NOT NULL,
                  page_no INTEGER,
                  start_offset INTEGER,
                  end_offset INTEGER,
                  text TEXT NOT NULL,
                  UNIQUE(attachment_id,chunk_no)
                );
                CREATE INDEX IF NOT EXISTS idx_attachment_chunks_attachment
                  ON attachment_chunks(attachment_id,chunk_no);
                CREATE VIRTUAL TABLE IF NOT EXISTS attachment_fts USING fts5(
                  owner_id UNINDEXED,
                  session_id UNINDEXED,
                  attachment_id UNINDEXED,
                  chunk_no UNINDEXED,
                  text
                );
                CREATE TRIGGER IF NOT EXISTS attachment_fts_insert AFTER INSERT ON attachment_chunks BEGIN
                  INSERT INTO attachment_fts(rowid,owner_id,session_id,attachment_id,chunk_no,text)
                  SELECT new.id,a.owner_id,a.session_id,new.attachment_id,new.chunk_no,new.text
                    FROM attachments a WHERE a.attachment_id=new.attachment_id;
                END;
                CREATE TRIGGER IF NOT EXISTS attachment_fts_update AFTER UPDATE ON attachment_chunks BEGIN
                  DELETE FROM attachment_fts WHERE rowid=old.id;
                  INSERT INTO attachment_fts(rowid,owner_id,session_id,attachment_id,chunk_no,text)
                  SELECT new.id,a.owner_id,a.session_id,new.attachment_id,new.chunk_no,new.text
                    FROM attachments a WHERE a.attachment_id=new.attachment_id;
                END;
                CREATE TRIGGER IF NOT EXISTS attachment_fts_delete AFTER DELETE ON attachment_chunks BEGIN
                  DELETE FROM attachment_fts WHERE rowid=old.id;
                END;
                INSERT INTO attachment_fts(rowid,owner_id,session_id,attachment_id,chunk_no,text)
                  SELECT c.id,a.owner_id,a.session_id,c.attachment_id,c.chunk_no,c.text
                    FROM attachment_chunks c JOIN attachments a ON a.attachment_id=c.attachment_id
                   WHERE NOT EXISTS(SELECT 1 FROM attachment_fts f WHERE f.rowid=c.id);
                INSERT OR IGNORE INTO schema_migrations(version) VALUES(16);
                "#,
            )?;
            // v0.2.7 preserves Unknown independently from Unsupported. Older
            // false booleans did not carry enough evidence, so migrate them to
            // unknown; only prior true values can safely become supported.
            ensure_column(conn, "provider_profile_models", "native_tools_state", "TEXT NOT NULL DEFAULT 'unknown'")?;
            ensure_column(conn, "provider_profile_models", "structured_output_state", "TEXT NOT NULL DEFAULT 'unknown'")?;
            ensure_column(conn, "provider_profile_models", "continuation_state", "TEXT NOT NULL DEFAULT 'unknown'")?;
            ensure_column(conn, "provider_profile_models", "vision_state", "TEXT NOT NULL DEFAULT 'unknown'")?;
            ensure_column(conn, "provider_profile_models", "file_input_state", "TEXT NOT NULL DEFAULT 'unknown'")?;
            conn.execute_batch(
                r#"
                UPDATE provider_profile_models SET native_tools_state='supported' WHERE native_tools=1 AND native_tools_state='unknown';
                UPDATE provider_profile_models SET structured_output_state='supported' WHERE structured_output=1 AND structured_output_state='unknown';
                UPDATE provider_profile_models SET continuation_state='supported' WHERE continuation=1 AND continuation_state='unknown';
                UPDATE provider_profile_models SET vision_state='supported' WHERE vision_capable=1 AND vision_state='unknown';
                UPDATE provider_profile_models SET file_input_state='supported' WHERE file_input_capable=1 AND file_input_state='unknown';
                INSERT OR IGNORE INTO schema_migrations(version) VALUES(17);
                "#,
            )?;
            Ok(())
        })
    }

    /// Resolve a Telegram user to Xiao's stable single-owner identity and
    /// transactionally rekey any v0.2.5 chat-scoped principals. Canonical
    /// USER.md/MEMORY.md and filesystem skills are reindexed by the caller
    /// when `requires_file_reconcile` is true.
    pub fn ensure_telegram_owner(&self, telegram_user_id: i64) -> Result<OwnerMigrationResult> {
        if telegram_user_id == 0 {
            return Err(anyhow::anyhow!("Telegram owner id must be non-zero"));
        }
        let owner_id = format!("owner:telegram:{telegram_user_id}");
        self.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let now = Utc::now().to_rfc3339();
            transaction.execute(
                "INSERT INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES(?,?,?,?) ON CONFLICT(owner_id) DO UPDATE SET telegram_user_id=excluded.telegram_user_id,updated_at=excluded.updated_at",
                params![owner_id, telegram_user_id, now, now],
            )?;
            // v0.2.5 accounts were installation-global and had no owner
            // binding. Xiao is single-owner, so the first stable owner claims
            // those rows; chat/topic IDs never become account ownership.
            transaction.execute(
                "UPDATE provider_accounts SET owner_id=? WHERE owner_id IS NULL",
                params![owner_id],
            )?;

            let suffix = format!("%:{telegram_user_id}");
            let mut legacy = std::collections::BTreeSet::new();
            for sql in [
                "SELECT DISTINCT owner_principal FROM sessions WHERE owner_principal LIKE ? AND owner_principal LIKE 'telegram:%'",
                "SELECT DISTINCT owner_principal FROM memories WHERE owner_principal LIKE ? AND owner_principal LIKE 'telegram:%'",
                "SELECT DISTINCT owner_principal FROM skills WHERE owner_principal LIKE ? AND owner_principal LIKE 'telegram:%'",
                "SELECT DISTINCT owner_principal FROM agent_runs WHERE owner_principal LIKE ? AND owner_principal LIKE 'telegram:%'",
            ] {
                let mut statement = transaction.prepare(sql)?;
                let values = statement
                    .query_map(params![suffix], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                legacy.extend(values);
            }
            // A headless/WebUI-first installation may have created its stable
            // installation owner before Telegram was configured. That row is
            // the same single human, not a second tenant, so claim all of its
            // durable state when the configured Telegram identity appears.
            // v0.2.6 could materialize more than one Telegram owner from
            // allowed_user_ids. Once v0.2.7 has an explicitly selected owner,
            // every other owner row is legacy state belonging to the same
            // installation and is transactionally folded into the canonical
            // owner. This also covers the headless owner:local case.
            {
                let mut statement = transaction.prepare(
                    "SELECT owner_id FROM owners WHERE owner_id<>? ORDER BY created_at,owner_id",
                )?;
                let values = statement
                    .query_map(params![owner_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                legacy.extend(values);
            }
            transaction.execute(
                "INSERT OR IGNORE INTO access_principals(principal,role,created_at,updated_at) VALUES(?,'owner',?,?)",
                params![owner_id, now, now],
            )?;
            if legacy.is_empty() {
                transaction.commit()?;
                return Ok(OwnerMigrationResult {
                    owner_id,
                    migrated_legacy_principals: 0,
                    requires_file_reconcile: false,
                });
            }

            // Active owner-global indexes are derived state. Canonical living
            // files win conflicts, so remove only their active rows and force
            // a clean file reconciliation after the transaction commits.
            transaction.execute(
                "DELETE FROM memories WHERE owner_principal=?",
                params![owner_id],
            )?;
            transaction.execute(
                "DELETE FROM skills WHERE owner_principal=?",
                params![owner_id],
            )?;
            for principal in &legacy {
                // Preserve both sides if a partially migrated database already
                // contains the same alias. Profile IDs are global; a stable
                // hash suffix gives the legacy profile a bounded, valid,
                // deterministic alias before owner rekeying.
                let conflicts = {
                    let mut statement = transaction.prepare(
                        "SELECT legacy.profile_id,legacy.alias FROM provider_profiles legacy WHERE legacy.owner_id=? AND EXISTS(SELECT 1 FROM provider_profiles current WHERE current.owner_id=? AND current.alias=legacy.alias)",
                    )?;
                    let rows = statement
                        .query_map(params![principal, owner_id], |row| {
                            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    rows
                };
                for (profile_id, alias) in conflicts {
                    let digest = format!("{:x}", Sha256::digest(profile_id.as_bytes()));
                    let suffix = &digest[..12];
                    let max_base = 64usize - "-legacy-".len() - suffix.len();
                    let base = alias.chars().take(max_base).collect::<String>();
                    let migrated_alias = format!("{base}-legacy-{suffix}");
                    transaction.execute(
                        "UPDATE provider_profiles SET alias=? WHERE owner_id=? AND profile_id=?",
                        params![migrated_alias, principal, profile_id],
                    )?;
                }
                transaction.execute(
                    "UPDATE provider_accounts SET owner_id=? WHERE owner_id=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE provider_profiles SET owner_id=? WHERE owner_id=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE attachments SET owner_id=? WHERE owner_id=?",
                    params![owner_id, principal],
                )?;
                // Attachment FTS denormalizes ownership and has no trigger on
                // its parent row, so rekey its filter column explicitly.
                transaction.execute(
                    "UPDATE attachment_fts SET owner_id=? WHERE owner_id=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE legacy_owner_principals SET owner_id=? WHERE owner_id=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "DELETE FROM memories WHERE owner_principal=?",
                    params![principal],
                )?;
                transaction.execute(
                    "DELETE FROM skills WHERE owner_principal=?",
                    params![principal],
                )?;
                transaction.execute(
                    "UPDATE memory_history SET owner_principal=? WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE skill_history SET owner_principal=? WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE session_summaries SET owner_principal=? WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE agent_runs SET owner_principal=? WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "UPDATE audit_events SET principal=? WHERE principal=?",
                    params![owner_id, principal],
                )?;
                // v0.2.5 grants lacked session/run/call binding. Preserve the
                // audit row but make every unconsumed legacy grant unusable.
                transaction.execute(
                    "UPDATE approvals SET owner_principal=?,status=CASE WHEN status IN ('pending','approved') THEN 'expired' ELSE status END WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;

                // Scope rows preserve the original chat/topic namespace. Use
                // upserts so an interrupted earlier migration is idempotent.
                transaction.execute(
                    "UPDATE telegram_session_scopes SET owner_principal=? WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "INSERT INTO telegram_active_sessions(owner_principal,chat_id,thread_id_key,active_main_session_id,side_session_id,mode,updated_at) SELECT ?,chat_id,thread_id_key,active_main_session_id,side_session_id,mode,updated_at FROM telegram_active_sessions WHERE owner_principal=? ON CONFLICT(owner_principal,chat_id,thread_id_key) DO UPDATE SET active_main_session_id=excluded.active_main_session_id,side_session_id=excluded.side_session_id,mode=excluded.mode,updated_at=excluded.updated_at WHERE excluded.updated_at>telegram_active_sessions.updated_at",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "DELETE FROM telegram_active_sessions WHERE owner_principal=?",
                    params![principal],
                )?;
                transaction.execute(
                    "UPDATE sessions SET owner_principal=? WHERE owner_principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "INSERT OR IGNORE INTO frontend_state(principal,active_main_session_id,side_session_id,mode) SELECT ?,active_main_session_id,side_session_id,mode FROM frontend_state WHERE principal=?",
                    params![owner_id, principal],
                )?;
                transaction.execute(
                    "DELETE FROM frontend_state WHERE principal=?",
                    params![principal],
                )?;
                transaction.execute(
                    "DELETE FROM access_principals WHERE principal=?",
                    params![principal],
                )?;
                transaction.execute(
                    "INSERT OR REPLACE INTO legacy_owner_principals(legacy_principal,owner_id,migrated_at) VALUES(?,?,?)",
                    params![principal, owner_id, now],
                )?;
            }
            for principal in &legacy {
                transaction.execute(
                    "DELETE FROM owners WHERE owner_id=? AND owner_id<>?",
                    params![principal, owner_id],
                )?;
            }
            transaction.execute("DELETE FROM workspace_file_index", [])?;
            transaction.commit()?;
            Ok(OwnerMigrationResult {
                owner_id,
                migrated_legacy_principals: legacy.len(),
                requires_file_reconcile: true,
            })
        })
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let run = || {
            let mut conn = self
                .conn
                .lock()
                .map_err(|_| anyhow::anyhow!("sqlite mutex poisoned"))?;
            f(&mut conn)
        };
        match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
            Ok(tokio::runtime::RuntimeFlavor::MultiThread) => tokio::task::block_in_place(run),
            _ => run(),
        }
    }

    pub fn health(&self) -> bool {
        self.with_conn(|conn| {
            conn.query_row("SELECT 1", [], |_| Ok(()))?;
            Ok(())
        })
        .is_ok()
    }

    pub fn diagnostic_transaction(&self) -> Result<()> {
        self.with_conn(|connection| {
            let transaction = connection.transaction()?;
            transaction.query_row("SELECT 1", [], |_| Ok(()))?;
            transaction.rollback()?;
            Ok(())
        })
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
                    [],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    /// Resolve the installation's single management owner without deriving
    /// identity from a chat/topic. A fresh non-Telegram installation gets a
    /// stable local owner row; once a Telegram owner exists it takes priority.
    pub fn management_owner_id(&self) -> Result<String> {
        self.with_conn(|connection| {
            let owners = {
                let mut statement = connection.prepare(
                    "SELECT owner_id FROM owners ORDER BY CASE WHEN telegram_user_id IS NULL THEN 1 ELSE 0 END,created_at,owner_id",
                )?;
                let rows = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            if owners.len() > 1 {
                return Err(anyhow::anyhow!(
                    "multiple legacy owners require explicit owner resolution"
                ));
            }
            if let Some(owner) = owners.into_iter().next() {
                return Ok(owner);
            }
            let owner = "owner:local".to_owned();
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES(?,NULL,?,?)",
                params![owner, now, now],
            )?;
            connection.execute(
                "INSERT OR IGNORE INTO access_principals(principal,role,created_at,updated_at) VALUES(?,'owner',?,?)",
                params![owner, now, now],
            )?;
            Ok(owner)
        })
    }

    pub fn manager_counts(&self, owner: &str) -> Result<ManagerCounts> {
        self.with_conn(|connection| {
            let count = |connection: &Connection, sql: &str| -> Result<usize> {
                Ok(connection.query_row(sql, params![owner], |row| {
                    row.get::<_, i64>(0)
                })?.max(0) as usize)
            };
            Ok(ManagerCounts {
                sessions: count(connection, "SELECT COUNT(*) FROM sessions WHERE owner_principal=?")?,
                messages: count(connection, "SELECT COUNT(*) FROM messages m JOIN sessions s ON s.id=m.session_id WHERE s.owner_principal=?")?,
                agent_runs: count(connection, "SELECT COUNT(*) FROM agent_runs WHERE owner_principal=?")?,
                running_runs: count(connection, "SELECT COUNT(*) FROM agent_runs WHERE owner_principal=? AND status IN ('received','context_build','running','awaiting_approval','verifying')")?,
                blocked_runs: count(connection, "SELECT COUNT(*) FROM agent_runs WHERE owner_principal=? AND status='blocked'")?,
                memories: count(connection, "SELECT COUNT(*) FROM memories WHERE owner_principal=?")?,
                skills: count(connection, "SELECT COUNT(*) FROM skills WHERE owner_principal=?")?,
                attachments: count(connection, "SELECT COUNT(*) FROM attachments WHERE owner_id=?")?,
                pending_approvals: count(connection, "SELECT COUNT(*) FROM approvals WHERE owner_principal=? AND status='pending'")?,
            })
        })
    }

    pub fn session_fts_health(&self) -> Result<usize> {
        self.with_conn(|connection| {
            let count: i64 =
                connection.query_row("SELECT COUNT(*) FROM messages_fts", [], |row| row.get(0))?;
            Ok(count.max(0) as usize)
        })
    }

    pub fn record_environment_probe(&self, snapshot_json: &str, probed_at: &str) -> Result<()> {
        serde_json::from_str::<serde_json::Value>(snapshot_json)
            .map_err(|_| anyhow::anyhow!("environment probe must be valid JSON"))?;
        self.with_conn(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO environment_probes(snapshot_json,probed_at) VALUES(?,?)",
                params![snapshot_json, probed_at],
            )?;
            transaction.execute(
                "DELETE FROM environment_probes WHERE id NOT IN (SELECT id FROM environment_probes ORDER BY id DESC LIMIT 100)",
                [],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn upsert_provider_capability(&self, record: &ProviderCapabilityRecord) -> Result<()> {
        if !matches!(
            record.tool_protocol.as_str(),
            "native" | "structured_json_fallback" | "chat_only"
        ) {
            return Err(anyhow::anyhow!("invalid provider tool protocol"));
        }
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO provider_capabilities(provider,model,tool_protocol,native_tool_calls,structured_output,continuation,probed_at,evidence) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(provider,model) DO UPDATE SET tool_protocol=excluded.tool_protocol,native_tool_calls=excluded.native_tool_calls,structured_output=excluded.structured_output,continuation=excluded.continuation,probed_at=excluded.probed_at,evidence=excluded.evidence",
                params![record.provider, record.model, record.tool_protocol, record.native_tool_calls as i32, record.structured_output as i32, record.continuation as i32, record.probed_at, record.evidence],
            )?;
            Ok(())
        })
    }

    pub fn provider_capability(
        &self,
        provider: &str,
        model: &str,
    ) -> Result<Option<ProviderCapabilityRecord>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT provider,model,tool_protocol,native_tool_calls,structured_output,continuation,probed_at,evidence FROM provider_capabilities WHERE provider=? AND model=?",
                    params![provider, model],
                    |row| {
                        Ok(ProviderCapabilityRecord {
                            provider: row.get(0)?,
                            model: row.get(1)?,
                            tool_protocol: row.get(2)?,
                            native_tool_calls: row.get::<_, i64>(3)? != 0,
                            structured_output: row.get::<_, i64>(4)? != 0,
                            continuation: row.get::<_, i64>(5)? != 0,
                            probed_at: row.get(6)?,
                            evidence: row.get(7)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn workspace_file_hash(&self, path: &str) -> Result<Option<String>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT content_hash FROM workspace_file_index WHERE path=?",
                    params![path],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn set_workspace_file_hash(&self, path: &str, kind: &str, hash: &str) -> Result<()> {
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO workspace_file_index(path,kind,content_hash,indexed_at) VALUES(?,?,?,?) ON CONFLICT(path) DO UPDATE SET kind=excluded.kind,content_hash=excluded.content_hash,indexed_at=excluded.indexed_at",
                params![path, kind, hash, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn skill_file_hash(&self, path: &str) -> Result<Option<String>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT content_hash FROM skill_file_index WHERE path=?",
                    params![path],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn set_skill_file_hash(&self, path: &str, name: &str, hash: &str) -> Result<()> {
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO skill_file_index(path,skill_name,content_hash,indexed_at) VALUES(?,?,?,?) ON CONFLICT(path) DO UPDATE SET skill_name=excluded.skill_name,content_hash=excluded.content_hash,indexed_at=excluded.indexed_at",
                params![path, name, hash, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn begin_dependency_install(&self, start: DependencyInstallStart<'_>) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.with_conn(|connection| {
            connection.execute(
                "INSERT INTO dependency_installs(id,agent_run_id,binary,package,package_manager,source,validated,requested_capability,status,started_at) VALUES(?,?,?,?,?,?,?,?,'installing',?)",
                params![id, start.agent_run_id, start.binary, start.package, start.package_manager, start.source, start.validated as i32, start.requested_capability, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn finish_dependency_install(&self, id: &str, status: &str, evidence: &str) -> Result<()> {
        if !matches!(status, "succeeded" | "failed" | "interrupted") {
            return Err(anyhow::anyhow!("invalid dependency install status"));
        }
        self.with_conn(|connection| {
            let changed = connection.execute(
                "UPDATE dependency_installs SET status=?,evidence=?,finished_at=? WHERE id=? AND status='installing'",
                params![status, evidence, Utc::now().to_rfc3339(), id],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("dependency install record not found or terminal"));
            }
            Ok(())
        })
    }

    pub fn dependency_installs(&self, agent_run_id: &str) -> Result<Vec<DependencyInstallRecord>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,agent_run_id,binary,package,package_manager,source,validated,requested_capability,status,evidence,started_at,finished_at FROM dependency_installs WHERE agent_run_id=? ORDER BY started_at",
            )?;
            let rows = statement.query_map(params![agent_run_id], |row| {
                Ok(DependencyInstallRecord {
                    id: row.get(0)?,
                    agent_run_id: row.get(1)?,
                    binary: row.get(2)?,
                    package: row.get(3)?,
                    package_manager: row.get(4)?,
                    source: row.get(5)?,
                    validated: row.get::<_, i64>(6)? != 0,
                    requested_capability: row.get(7)?,
                    status: row.get(8)?,
                    evidence: row.get(9)?,
                    started_at: row.get(10)?,
                    finished_at: row.get(11)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn request_approval(&self, request: ApprovalRequest<'_>) -> Result<ApprovalRecord> {
        let binding = request.binding;
        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let expires_at = (now + chrono::Duration::minutes(15)).to_rfc3339();
        self.with_conn(|connection| {
            let pending_sql = APPROVAL_SELECT.to_owned() + " WHERE owner_principal=? AND session_id=? AND agent_run_id=? AND tool_call_id=? AND tool_name=? AND arguments_hash=? AND status='pending' AND expires_at>? ORDER BY requested_at DESC LIMIT 1";
            if let Some(record) = connection
                .query_row(
                    &pending_sql,
                    params![binding.owner_id, binding.session_id, binding.agent_run_id, binding.tool_call_id, binding.tool_name, binding.arguments_hash, now_text],
                    row_approval,
                )
                .optional()?
            {
                return Ok(record);
            }
            let id = Uuid::new_v4().to_string();
            connection.execute(
                "INSERT INTO approvals(id,owner_principal,session_id,agent_run_id,tool_call_id,capability,tool_name,arguments_hash,risk,summary,status,approval_mode,requested_at,expires_at) VALUES(?,?,?,?,?,?,?,?,?,?,'pending','explicit',?,?)",
                params![id, binding.owner_id, binding.session_id, binding.agent_run_id, binding.tool_call_id, request.capability, binding.tool_name, binding.arguments_hash, request.risk, request.summary, now_text, expires_at],
            )?;
            let select_sql = APPROVAL_SELECT.to_owned() + " WHERE id=?";
            connection.query_row(
                &select_sql,
                params![id],
                row_approval,
            ).map_err(Into::into)
        })
    }

    pub fn decide_approval(&self, owner: &str, id: &str, approve: bool) -> Result<bool> {
        self.with_conn(|connection| {
            let changed = connection.execute(
                "UPDATE approvals SET status=?,approval_mode='explicit',decided_at=? WHERE id=? AND owner_principal=? AND status='pending' AND expires_at>?",
                params![if approve { "approved" } else { "denied" }, Utc::now().to_rfc3339(), id, owner, Utc::now().to_rfc3339()],
            )?;
            Ok(changed == 1)
        })
    }

    pub fn consume_approval(&self, binding: ApprovalBinding<'_>) -> Result<bool> {
        self.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let id = transaction
                .query_row(
                    "SELECT id FROM approvals WHERE owner_principal=? AND session_id=? AND agent_run_id=? AND tool_call_id=? AND tool_name=? AND arguments_hash=? AND status='approved' AND consumed_at IS NULL AND expires_at>? ORDER BY decided_at DESC LIMIT 1",
                    params![binding.owner_id, binding.session_id, binding.agent_run_id, binding.tool_call_id, binding.tool_name, binding.arguments_hash, Utc::now().to_rfc3339()],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let consumed = if let Some(id) = id {
                transaction.execute(
                    "UPDATE approvals SET status='consumed',consumed_at=? WHERE id=? AND status='approved' AND consumed_at IS NULL",
                    params![Utc::now().to_rfc3339(), id],
                )? == 1
            } else {
                false
            };
            transaction.commit()?;
            Ok(consumed)
        })
    }

    pub fn approval_status(&self, binding: ApprovalBinding<'_>) -> Result<Option<String>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT status FROM approvals WHERE owner_principal=? AND session_id=? AND agent_run_id=? AND tool_call_id=? AND tool_name=? AND arguments_hash=? ORDER BY requested_at DESC LIMIT 1",
                    params![binding.owner_id, binding.session_id, binding.agent_run_id, binding.tool_call_id, binding.tool_name, binding.arguments_hash],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn pending_approvals(&self, owner: &str) -> Result<Vec<ApprovalRecord>> {
        self.pending_approvals_for_session(owner, None)
    }

    pub fn pending_approvals_for_session(
        &self,
        owner: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<ApprovalRecord>> {
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE approvals SET status='expired' WHERE status IN ('pending','approved') AND expires_at<=?",
                params![Utc::now().to_rfc3339()],
            )?;
            let sql = APPROVAL_SELECT.to_owned()
                + if session_id.is_some() {
                    " WHERE owner_principal=? AND session_id=? AND status='pending' ORDER BY requested_at DESC LIMIT 100"
                } else {
                    " WHERE owner_principal=? AND status='pending' ORDER BY requested_at DESC LIMIT 100"
                };
            let mut statement = connection.prepare(&sql)?;
            let rows = if let Some(session_id) = session_id {
                statement.query_map(params![owner, session_id], row_approval)?
            } else {
                statement.query_map(params![owner], row_approval)?
            };
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
    pub fn checkpoint(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    /// On daemon restart, an in-flight run is an uncertainty boundary. It is
    /// made terminal instead of being replayed, especially for side effects.
    fn recover_interrupted_runs(&self) -> Result<()> {
        self.with_conn(|conn| {
            let now = Utc::now().to_rfc3339();
            let tx = conn.transaction()?;
            tx.execute(
                "UPDATE tool_runs SET status='interrupted',finished_at=?,error=COALESCE(error,'daemon stopped during tool execution') WHERE status IN ('requested','policy_check','installing_dependency','running')",
                params![now],
            )?;
            tx.execute(
                "UPDATE agent_runs SET status='interrupted',finished_at=?,error=COALESCE(error,'daemon stopped during agent run') WHERE status IN ('received','context_build','running','verifying')",
                params![now],
            )?;
            tx.execute(
                "UPDATE dependency_installs SET status='interrupted',finished_at=?,evidence=COALESCE(evidence,'daemon stopped during package installation') WHERE status='installing'",
                params![now],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn create_agent_run(
        &self,
        owner: &str,
        session_id: &str,
        provider: &str,
        model: &str,
        goal: Option<&str>,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            let owns_session: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=?)",
                params![session_id, owner],
                |row| row.get(0),
            )?;
            if !owns_session {
                return Err(anyhow::anyhow!("session not found for principal"));
            }
            conn.execute(
                "INSERT INTO agent_runs(id,owner_principal,session_id,provider,model,status,goal,started_at) VALUES(?,?,?,?,?,'running',?,?)",
                params![id, owner, session_id, provider, model, goal, now],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn set_agent_run_status(
        &self,
        owner: &str,
        run_id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        if !matches!(
            status,
            "received"
                | "context_build"
                | "running"
                | "awaiting_approval"
                | "verifying"
                | "completed"
                | "blocked"
                | "failed"
                | "cancelled"
                | "interrupted"
        ) {
            return Err(anyhow::anyhow!("invalid agent run status"));
        }
        let terminal = matches!(
            status,
            "completed" | "blocked" | "failed" | "cancelled" | "interrupted"
        );
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE agent_runs SET status=?,error=?,finished_at=CASE WHEN ? THEN ? ELSE NULL END WHERE id=? AND owner_principal=?",
                params![status, error, terminal, Utc::now().to_rfc3339(), run_id, owner],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("agent run not found for principal"));
            }
            Ok(())
        })
    }

    pub fn set_agent_run_model(&self, owner: &str, run_id: &str, model: &str) -> Result<()> {
        self.with_conn(|connection| {
            let changed = connection.execute(
                "UPDATE agent_runs SET model=? WHERE id=? AND owner_principal=? AND status='running'",
                params![model, run_id, owner],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("running agent run not found for principal"));
            }
            Ok(())
        })
    }

    pub fn agent_run(&self, owner: &str, run_id: &str) -> Result<Option<AgentRunRecord>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id,owner_principal,session_id,provider,model,status,goal,started_at,finished_at,error FROM agent_runs WHERE id=? AND owner_principal=?",
                params![run_id, owner],
                |row| {
                    Ok(AgentRunRecord {
                        id: row.get(0)?,
                        owner_principal: row.get(1)?,
                        session_id: row.get(2)?,
                        provider: row.get(3)?,
                        model: row.get(4)?,
                        status: row.get(5)?,
                        goal: row.get(6)?,
                        started_at: row.get(7)?,
                        finished_at: row.get(8)?,
                        error: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })
    }

    pub fn agent_runs(&self, owner: &str, limit: usize) -> Result<Vec<AgentRunRecord>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,owner_principal,session_id,provider,model,status,goal,started_at,finished_at,error FROM agent_runs WHERE owner_principal=? ORDER BY started_at DESC LIMIT ?",
            )?;
            let rows = statement.query_map(params![owner, limit.clamp(1, 500) as i64], |row| {
                Ok(AgentRunRecord {
                    id: row.get(0)?,
                    owner_principal: row.get(1)?,
                    session_id: row.get(2)?,
                    provider: row.get(3)?,
                    model: row.get(4)?,
                    status: row.get(5)?,
                    goal: row.get(6)?,
                    started_at: row.get(7)?,
                    finished_at: row.get(8)?,
                    error: row.get(9)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// Return an assistant result persisted within this run's observable time
    /// window, so a later answer in the same session is never misattributed.
    pub fn agent_run_result(&self, owner: &str, run: &AgentRunRecord) -> Result<Option<String>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT m.content FROM messages m JOIN sessions s ON s.id=m.session_id WHERE m.session_id=? AND s.owner_principal=? AND m.role='assistant' AND m.created_at>=? AND (? IS NULL OR m.created_at<=?) ORDER BY m.id DESC LIMIT 1",
                    params![run.session_id, owner, run.started_at, run.finished_at, run.finished_at],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn create_tool_run(
        &self,
        agent_run_id: &str,
        call_id: &str,
        tool_name: &str,
        arguments_json: &str,
        risk: &str,
    ) -> Result<String> {
        if call_id.trim().is_empty() || call_id.chars().count() > 256 {
            return Err(anyhow::anyhow!("tool call id is empty or too long"));
        }
        if tool_name.trim().is_empty() || tool_name.chars().count() > 128 {
            return Err(anyhow::anyhow!("tool name is empty or too long"));
        }
        serde_json::from_str::<serde_json::Value>(arguments_json)
            .map_err(|_| anyhow::anyhow!("tool audit arguments must be valid JSON"))?;
        let id = Uuid::new_v4().to_string();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO tool_runs(id,agent_run_id,call_id,tool_name,arguments_json,risk,status) VALUES(?,?,?,?,?,?,'requested')",
                params![id, agent_run_id, call_id, tool_name, arguments_json, risk],
            )?;
            Ok(())
        })?;
        Ok(id)
    }

    pub fn set_tool_run_status(
        &self,
        tool_run_id: &str,
        status: &str,
        output: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        if !matches!(
            status,
            "requested"
                | "policy_check"
                | "awaiting_approval"
                | "installing_dependency"
                | "running"
                | "succeeded"
                | "failed"
                | "interrupted"
                | "denied"
        ) {
            return Err(anyhow::anyhow!("invalid tool run status"));
        }
        let starting = status == "running";
        let terminal = matches!(status, "succeeded" | "failed" | "interrupted" | "denied");
        self.with_conn(|conn| {
            let changed = conn.execute(
                "UPDATE tool_runs SET status=?,output=?,error=?,started_at=CASE WHEN ? THEN COALESCE(started_at,?) ELSE started_at END,finished_at=CASE WHEN ? THEN ? ELSE finished_at END WHERE id=?",
                params![
                    status,
                    output,
                    error,
                    starting,
                    Utc::now().to_rfc3339(),
                    terminal,
                    Utc::now().to_rfc3339(),
                    tool_run_id
                ],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("tool run not found"));
            }
            Ok(())
        })
    }

    pub fn set_tool_run_approval_audit(
        &self,
        tool_run_id: &str,
        approval_mode: Option<&str>,
        policy_original: Option<&str>,
    ) -> Result<()> {
        self.with_conn(|connection| {
            let changed = connection.execute(
                "UPDATE tool_runs SET approval_mode=?,policy_original=? WHERE id=?",
                params![approval_mode, policy_original, tool_run_id],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("tool run not found"));
            }
            Ok(())
        })
    }

    pub fn tool_runs(&self, owner: &str, agent_run_id: &str) -> Result<Vec<ToolRunRecord>> {
        self.with_conn(|conn| {
            let mut statement = conn.prepare(
                "SELECT t.id,t.agent_run_id,t.call_id,t.tool_name,t.arguments_json,t.risk,t.approval_mode,t.policy_original,t.status,t.output,t.error,t.started_at,t.finished_at FROM tool_runs t JOIN agent_runs a ON a.id=t.agent_run_id WHERE t.agent_run_id=? AND a.owner_principal=? ORDER BY t.rowid",
            )?;
            let rows = statement.query_map(params![agent_run_id, owner], |row| {
                Ok(ToolRunRecord {
                    id: row.get(0)?,
                    agent_run_id: row.get(1)?,
                    call_id: row.get(2)?,
                    tool_name: row.get(3)?,
                    arguments_json: row.get(4)?,
                    risk: row.get(5)?,
                    approval_mode: row.get(6)?,
                    policy_original: row.get(7)?,
                    status: row.get(8)?,
                    output: row.get(9)?,
                    error: row.get(10)?,
                    started_at: row.get(11)?,
                    finished_at: row.get(12)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn create_session(
        &self,
        owner: &str,
        name: &str,
        provider: &str,
        account_id: Option<&str>,
        model: &str,
        is_side: bool,
        parent_id: Option<&str>,
    ) -> Result<SessionRecord> {
        if owner.trim().is_empty() {
            return Err(anyhow::anyhow!("session owner must not be empty"));
        }
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            if let Some(parent) = parent_id {
                let ok: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=? AND is_side=0)",
                    params![parent, owner], |r| r.get(0)
                )?;
                if !ok { return Err(anyhow::anyhow!("parent session is not owned by principal")); }
            }
            conn.execute(
                "INSERT INTO sessions(id,name,provider,account_id,model,is_side,parent_id,created_at,last_active_at,owner_principal) VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![id,name,provider,account_id,model,is_side as i32,parent_id,now,now,owner],
            )?;
            Ok(())
        })?;
        self.session(owner, &id)?.context("created session missing")
    }

    pub fn session(&self, owner: &str, id: &str) -> Result<Option<SessionRecord>> {
        self.with_conn(|conn| conn.query_row(
            "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.yolo_mode,s.created_at,s.last_active_at FROM sessions s WHERE s.id=? AND s.owner_principal=?",
            params![id,owner], row_session,
        ).optional().map_err(Into::into))
    }

    pub fn list_main_sessions(
        &self,
        owner: &str,
        limit: usize,
        offset: usize,
        include_archived: bool,
    ) -> Result<Vec<SessionRecord>> {
        self.with_conn(|conn| {
            let sql = if include_archived {
                "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.yolo_mode,s.created_at,s.last_active_at FROM sessions s WHERE owner_principal=? AND is_side=0 ORDER BY last_active_at DESC LIMIT ? OFFSET ?"
            } else {
                "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.yolo_mode,s.created_at,s.last_active_at FROM sessions s WHERE owner_principal=? AND is_side=0 AND archived=0 ORDER BY last_active_at DESC LIMIT ? OFFSET ?"
            };
            let mut stmt = conn.prepare(sql)?;
            let rows = stmt.query_map(params![owner, limit as i64, offset as i64], row_session)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn count_main_sessions(&self, owner: &str) -> Result<usize> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
            "SELECT COUNT(*) FROM sessions WHERE owner_principal=? AND is_side=0 AND archived=0",
            params![owner], |r| r.get::<_, i64>(0)
        )? as usize)
        })
    }

    pub fn rename_session(&self, owner: &str, id: &str, name: &str) -> Result<()> {
        self.with_conn(|conn| {
            let n = conn.execute(
                "UPDATE sessions SET name=?,last_active_at=? WHERE id=? AND owner_principal=?",
                params![name, Utc::now().to_rfc3339(), id, owner],
            )?;
            if n != 1 {
                return Err(anyhow::anyhow!("session not found for principal"));
            }
            Ok(())
        })
    }
    pub fn archive_session(&self, owner: &str, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            let n=conn.execute("UPDATE sessions SET archived=1,last_active_at=? WHERE id=? AND owner_principal=? AND is_side=0", params![Utc::now().to_rfc3339(),id,owner])?;
            if n != 1 { return Err(anyhow::anyhow!("session not found for principal")); }
            Ok(())
        })
    }
    pub fn set_session_provider(
        &self,
        owner: &str,
        id: &str,
        provider: &str,
        account: Option<&str>,
        model: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let n=conn.execute("UPDATE sessions SET provider=?,account_id=?,model=?,last_active_at=? WHERE id=? AND owner_principal=?", params![provider,account,model,Utc::now().to_rfc3339(),id,owner])?;
            if n != 1 { return Err(anyhow::anyhow!("session not found for principal")); }
            Ok(())
        })
    }

    pub fn set_session_yolo(&self, owner: &str, id: &str, enabled: bool) -> Result<()> {
        self.with_conn(|connection| {
            let changed = connection.execute(
                "UPDATE sessions SET yolo_mode=?,last_active_at=? WHERE id=? AND owner_principal=? AND archived=0",
                params![enabled as i32, Utc::now().to_rfc3339(), id, owner],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("active session not found for principal"));
            }
            Ok(())
        })
    }

    pub fn telegram_scope_for_session(
        &self,
        owner: &str,
        session_id: &str,
    ) -> Result<Option<(i64, Option<i64>)>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT chat_id,thread_id_key FROM telegram_session_scopes WHERE owner_principal=? AND session_id=?",
                    params![owner, session_id],
                    |row| {
                        let chat_id = row.get(0)?;
                        let thread_key: i64 = row.get(1)?;
                        Ok((chat_id, (thread_key != 0).then_some(thread_key)))
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn bind_session_to_telegram_scope(
        &self,
        owner: &str,
        session_id: &str,
        scope: TelegramScope,
    ) -> Result<()> {
        self.with_conn(|connection| {
            let row = connection
                .query_row(
                    "SELECT is_side FROM sessions WHERE id=? AND owner_principal=?",
                    params![session_id, owner],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let Some(is_side) = row else {
                return Err(anyhow::anyhow!("session not found for principal"));
            };
            let existing = connection
                .query_row(
                    "SELECT chat_id,thread_id_key FROM telegram_session_scopes WHERE session_id=?",
                    params![session_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?;
            if let Some(existing) = existing {
                if existing != (scope.chat_id, scope.thread_key()) {
                    return Err(anyhow::anyhow!(
                        "session is already bound to another Telegram scope"
                    ));
                }
                return Ok(());
            }
            connection.execute(
                "INSERT INTO telegram_session_scopes(session_id,owner_principal,chat_id,thread_id_key,is_side,created_at) VALUES(?,?,?,?,?,?)",
                params![session_id, owner, scope.chat_id, scope.thread_key(), is_side, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn list_main_sessions_in_telegram_scope(
        &self,
        owner: &str,
        scope: TelegramScope,
        limit: usize,
        offset: usize,
        include_archived: bool,
    ) -> Result<Vec<SessionRecord>> {
        self.with_conn(|connection| {
            let archived = if include_archived { "" } else { " AND s.archived=0" };
            let sql = format!(
                "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.yolo_mode,s.created_at,s.last_active_at FROM sessions s JOIN telegram_session_scopes ts ON ts.session_id=s.id WHERE s.owner_principal=? AND ts.owner_principal=? AND ts.chat_id=? AND ts.thread_id_key=? AND s.is_side=0{archived} ORDER BY s.last_active_at DESC LIMIT ? OFFSET ?"
            );
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(
                params![
                    owner,
                    owner,
                    scope.chat_id,
                    scope.thread_key(),
                    limit as i64,
                    offset as i64
                ],
                row_session,
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn count_main_sessions_in_telegram_scope(
        &self,
        owner: &str,
        scope: TelegramScope,
    ) -> Result<usize> {
        self.with_conn(|connection| {
            Ok(connection.query_row(
                "SELECT COUNT(*) FROM sessions s JOIN telegram_session_scopes ts ON ts.session_id=s.id WHERE s.owner_principal=? AND ts.owner_principal=? AND ts.chat_id=? AND ts.thread_id_key=? AND s.is_side=0 AND s.archived=0",
                params![owner, owner, scope.chat_id, scope.thread_key()],
                |row| row.get::<_, i64>(0),
            )? as usize)
        })
    }

    pub fn set_telegram_frontend_state(
        &self,
        owner: &str,
        scope: TelegramScope,
        main: &str,
        side: Option<&str>,
        mode: &str,
    ) -> Result<()> {
        if !matches!(mode, "main" | "side") {
            return Err(anyhow::anyhow!("invalid Telegram session mode"));
        }
        self.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let main_ok: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions s JOIN telegram_session_scopes ts ON ts.session_id=s.id WHERE s.id=? AND s.owner_principal=? AND ts.owner_principal=? AND ts.chat_id=? AND ts.thread_id_key=? AND s.is_side=0 AND s.archived=0)",
                params![main, owner, owner, scope.chat_id, scope.thread_key()],
                |row| row.get(0),
            )?;
            if !main_ok {
                return Err(anyhow::anyhow!("main session is not owned by Telegram scope"));
            }
            if let Some(side_id) = side {
                let side_ok: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions s JOIN telegram_session_scopes ts ON ts.session_id=s.id WHERE s.id=? AND s.owner_principal=? AND ts.owner_principal=? AND ts.chat_id=? AND ts.thread_id_key=? AND s.is_side=1 AND s.parent_id=?)",
                    params![side_id, owner, owner, scope.chat_id, scope.thread_key(), main],
                    |row| row.get(0),
                )?;
                if !side_ok {
                    return Err(anyhow::anyhow!("side session is not owned by Telegram scope/main"));
                }
            }
            transaction.execute(
                "INSERT INTO telegram_active_sessions(owner_principal,chat_id,thread_id_key,active_main_session_id,side_session_id,mode,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(owner_principal,chat_id,thread_id_key) DO UPDATE SET active_main_session_id=excluded.active_main_session_id,side_session_id=excluded.side_session_id,mode=excluded.mode,updated_at=excluded.updated_at",
                params![owner, scope.chat_id, scope.thread_key(), main, side, mode, Utc::now().to_rfc3339()],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn telegram_frontend_state(
        &self,
        owner: &str,
        scope: TelegramScope,
    ) -> Result<Option<(String, Option<String>, String)>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT active_main_session_id,side_session_id,mode FROM telegram_active_sessions WHERE owner_principal=? AND chat_id=? AND thread_id_key=?",
                    params![owner, scope.chat_id, scope.thread_key()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn reconcile_provider_models(
        &self,
        provider: &str,
        previous_default: Option<&str>,
        preferred_model: &str,
        valid_models: &[String],
    ) -> Result<usize> {
        if preferred_model.trim().is_empty() {
            return Err(anyhow::anyhow!(
                "preferred provider model must not be empty"
            ));
        }
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let sessions = {
                let mut statement = tx.prepare("SELECT id,model FROM sessions WHERE provider=?")?;
                let rows = statement.query_map(params![provider], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let previous_default = previous_default
                .map(str::trim)
                .filter(|model| !model.is_empty() && *model != preferred_model);
            let mut changed = 0;
            for (session_id, model) in sessions {
                let invalid = !valid_models.iter().any(|candidate| candidate == &model);
                let inherited_previous_default =
                    previous_default.is_some_and(|previous| previous == model.as_str());
                if model == "default" || invalid || inherited_previous_default {
                    changed += tx.execute(
                        "UPDATE sessions SET model=? WHERE id=? AND provider=?",
                        params![preferred_model, session_id, provider],
                    )?;
                }
            }
            tx.commit()?;
            Ok(changed)
        })
    }

    pub fn activate_account(
        &self,
        owner: &str,
        session_id: &str,
        account_id: &str,
        provider: &str,
        model: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let account_ok: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_accounts WHERE id=? AND provider=? AND status='connected' AND (owner_id=? OR owner_id IS NULL))",
                params![account_id,provider,owner], |r| r.get(0)
            )?;
            if !account_ok { return Err(anyhow::anyhow!("account is missing, disconnected, or belongs to another provider")); }
            let changed = tx.execute(
                "UPDATE sessions SET provider=?,account_id=?,model=?,last_active_at=? WHERE id=? AND owner_principal=?",
                params![provider,account_id,model,Utc::now().to_rfc3339(),session_id,owner]
            )?;
            if changed != 1 { return Err(anyhow::anyhow!("session not found for principal")); }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn append_message(
        &self,
        owner: &str,
        session_id: &str,
        role: &str,
        content: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=?)",
                params![session_id, owner],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(anyhow::anyhow!("session not found for principal"));
            }
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "INSERT INTO messages(session_id,role,content,created_at) VALUES(?,?,?,?)",
                params![session_id, role, content, now],
            )?;
            tx.execute(
                "UPDATE sessions SET last_active_at=? WHERE id=? AND owner_principal=?",
                params![Utc::now().to_rfc3339(), session_id, owner],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn latest_user_message(&self, owner: &str, session_id: &str) -> Result<Option<String>> {
        self.with_conn(|conn| conn.query_row(
            "SELECT m.content FROM messages m JOIN sessions s ON s.id=m.session_id WHERE m.session_id=? AND s.owner_principal=? AND m.role='user' ORDER BY m.id DESC LIMIT 1",
            params![session_id,owner], |r| r.get(0)
        ).optional().map_err(Into::into))
    }

    pub fn messages(&self, owner: &str, session_id: &str) -> Result<Vec<MessageRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT m.role,m.content,m.created_at FROM messages m JOIN sessions s ON s.id=m.session_id WHERE m.session_id=? AND s.owner_principal=? ORDER BY m.id")?;
            let rows = stmt.query_map(params![session_id,owner], |r| Ok(MessageRecord{role:r.get(0)?,content:r.get(1)?,created_at:r.get(2)?}))?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn stored_messages(
        &self,
        owner: &str,
        session_id: &str,
    ) -> Result<Vec<StoredMessageRecord>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT m.id,m.session_id,m.role,m.content,m.created_at FROM messages m JOIN sessions s ON s.id=m.session_id WHERE m.session_id=? AND s.owner_principal=? ORDER BY m.id",
            )?;
            let rows = statement.query_map(params![session_id, owner], |row| {
                Ok(StoredMessageRecord {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn session_summary(
        &self,
        owner: &str,
        session_id: &str,
    ) -> Result<Option<SessionSummaryRecord>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT session_id,owner_principal,summary,covered_through_message_id,created_at,updated_at FROM session_summaries WHERE session_id=? AND owner_principal=?",
                    params![session_id, owner],
                    |row| {
                        Ok(SessionSummaryRecord {
                            session_id: row.get(0)?,
                            owner_principal: row.get(1)?,
                            summary: row.get(2)?,
                            covered_through_message_id: row.get(3)?,
                            created_at: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn upsert_session_summary(
        &self,
        owner: &str,
        session_id: &str,
        summary: &str,
        covered_through_message_id: i64,
    ) -> Result<()> {
        if summary.trim().is_empty() || summary.chars().count() > 8_192 {
            return Err(anyhow::anyhow!("session summary is empty or too long"));
        }
        self.with_conn(|connection| {
            let owns_session: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=?)",
                params![session_id, owner],
                |row| row.get(0),
            )?;
            if !owns_session {
                return Err(anyhow::anyhow!("session not found for principal"));
            }
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO session_summaries(session_id,owner_principal,summary,covered_through_message_id,created_at,updated_at) VALUES(?,?,?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET summary=excluded.summary,covered_through_message_id=excluded.covered_through_message_id,updated_at=excluded.updated_at WHERE session_summaries.owner_principal=excluded.owner_principal",
                params![session_id, owner, summary, covered_through_message_id, now, now],
            )?;
            Ok(())
        })
    }

    pub fn set_frontend_state(
        &self,
        principal: &str,
        main: &str,
        side: Option<&str>,
        mode: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let transaction = conn.transaction()?;
            let main_ok: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=? AND is_side=0 AND archived=0)",
                params![main,principal], |r| r.get(0)
            )?;
            if !main_ok { return Err(anyhow::anyhow!("main session is not owned by principal")); }
            if let Some(side_id) = side {
                let side_ok: bool = transaction.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=? AND is_side=1 AND parent_id=?)",
                    params![side_id,principal,main], |r| r.get(0)
                )?;
                if !side_ok { return Err(anyhow::anyhow!("side session is not owned by principal/main")); }
            }
            transaction.execute(
                "INSERT INTO frontend_state(principal,active_main_session_id,side_session_id,mode) VALUES(?,?,?,?) ON CONFLICT(principal) DO UPDATE SET active_main_session_id=excluded.active_main_session_id,side_session_id=excluded.side_session_id,mode=excluded.mode",
                params![principal,main,side,mode],
            )?;
            if let Some((chat_id, thread_id_key)) = telegram_scope_from_principal(principal) {
                transaction.execute(
                    "INSERT INTO telegram_active_sessions(owner_principal,chat_id,thread_id_key,active_main_session_id,side_session_id,mode,updated_at) VALUES(?,?,?,?,?,?,?) ON CONFLICT(owner_principal,chat_id,thread_id_key) DO UPDATE SET active_main_session_id=excluded.active_main_session_id,side_session_id=excluded.side_session_id,mode=excluded.mode,updated_at=excluded.updated_at",
                    params![principal, chat_id, thread_id_key, main, side, mode, Utc::now().to_rfc3339()],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn frontend_state(
        &self,
        principal: &str,
    ) -> Result<Option<(String, Option<String>, String)>> {
        self.with_conn(|conn| conn.query_row(
            "SELECT active_main_session_id,side_session_id,mode FROM frontend_state WHERE principal=?",
            params![principal], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))
        ).optional().map_err(Into::into))
    }

    pub fn upsert_account(&self, a: &AccountRecord) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO provider_accounts(id,provider,label,email,status,access_expires_at,metadata_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(id) DO UPDATE SET provider=excluded.provider,label=excluded.label,email=excluded.email,status=excluded.status,access_expires_at=excluded.access_expires_at,metadata_json=excluded.metadata_json,updated_at=excluded.updated_at",
                params![a.id,a.provider,a.label,a.email,a.status,a.access_expires_at,a.metadata_json,now,now]
            )?;
            Ok(())
        })
    }

    pub fn set_account_owner(&self, owner: &str, account_id: &str) -> Result<()> {
        self.with_conn(|connection| {
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT OR IGNORE INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES(?,NULL,?,?)",
                params![owner, now, now],
            )?;
            let changed = connection.execute(
                "UPDATE provider_accounts SET owner_id=? WHERE id=? AND (owner_id IS NULL OR owner_id=?)",
                params![owner, account_id, owner],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("account not found or belongs to another owner"));
            }
            Ok(())
        })
    }

    pub fn account(&self, id: &str) -> Result<Option<AccountRecord>> {
        self.with_conn(|conn| conn.query_row(
            "SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts WHERE id=?",
            params![id],
            |r| Ok(AccountRecord{id:r.get(0)?,provider:r.get(1)?,label:r.get(2)?,email:r.get(3)?,status:r.get(4)?,access_expires_at:r.get(5)?,metadata_json:r.get(6)?})
        ).optional().map_err(Into::into))
    }

    pub fn account_for_owner(&self, owner: &str, id: &str) -> Result<Option<AccountRecord>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts WHERE id=? AND (owner_id=? OR owner_id IS NULL)",
                    params![id, owner],
                    |row| {
                        Ok(AccountRecord {
                            id: row.get(0)?,
                            provider: row.get(1)?,
                            label: row.get(2)?,
                            email: row.get(3)?,
                            status: row.get(4)?,
                            access_expires_at: row.get(5)?,
                            metadata_json: row.get(6)?,
                        })
                    },
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn accounts(&self, provider: Option<&str>) -> Result<Vec<AccountRecord>> {
        self.with_conn(|conn| {
            let (sql, p) = if provider.is_some() {
                ("SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts WHERE provider=? ORDER BY updated_at DESC", provider)
            } else {
                ("SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts ORDER BY updated_at DESC", None)
            };
            let mut stmt = conn.prepare(sql)?;
            let map = |r: &rusqlite::Row<'_>| Ok(AccountRecord{id:r.get(0)?,provider:r.get(1)?,label:r.get(2)?,email:r.get(3)?,status:r.get(4)?,access_expires_at:r.get(5)?,metadata_json:r.get(6)?});
            if let Some(v)=p { Ok(stmt.query_map(params![v], map)?.collect::<rusqlite::Result<Vec<_>>>()?) }
            else { Ok(stmt.query_map([], map)?.collect::<rusqlite::Result<Vec<_>>>()?) }
        })
    }

    pub fn accounts_for_owner(
        &self,
        owner: &str,
        provider: Option<&str>,
    ) -> Result<Vec<AccountRecord>> {
        self.with_conn(|connection| {
            let sql = if provider.is_some() {
                "SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts WHERE (owner_id=? OR owner_id IS NULL) AND provider=? ORDER BY updated_at DESC"
            } else {
                "SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts WHERE owner_id=? OR owner_id IS NULL ORDER BY updated_at DESC"
            };
            let mut statement = connection.prepare(sql)?;
            let row = |row: &rusqlite::Row<'_>| {
                Ok(AccountRecord {
                    id: row.get(0)?,
                    provider: row.get(1)?,
                    label: row.get(2)?,
                    email: row.get(3)?,
                    status: row.get(4)?,
                    access_expires_at: row.get(5)?,
                    metadata_json: row.get(6)?,
                })
            };
            let records = if let Some(provider) = provider {
                statement
                    .query_map(params![owner, provider], row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                statement
                    .query_map(params![owner], row)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(records)
        })
    }

    pub fn delete_account(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM provider_accounts WHERE id=?", params![id])?;
            Ok(())
        })
    }

    pub fn detach_account_from_sessions(&self, id: &str) -> Result<usize> {
        self.with_conn(|connection| {
            Ok(connection.execute(
                "UPDATE sessions SET account_id=NULL WHERE account_id=?",
                params![id],
            )?)
        })
    }

    pub fn session_attachment_bytes(&self, owner: &str, session_id: &str) -> Result<u64> {
        self.with_conn(|connection| {
            let value: i64 = connection.query_row(
                "SELECT COALESCE(SUM(size_bytes),0) FROM attachments WHERE owner_id=? AND session_id=? AND processing_status NOT IN ('rejected','failed')",
                params![owner, session_id],
                |row| row.get(0),
            )?;
            Ok(value.max(0) as u64)
        })
    }

    /// Raw bytes accounted to an owner. Failed/rejected rows remain counted when
    /// an explicit retention policy kept their files. Rows are removed when raw
    /// files are purged, so there is no unaccounted retained garbage.
    pub fn owner_attachment_bytes(&self, owner: &str) -> Result<u64> {
        self.with_conn(|connection| {
            let value: i64 = connection.query_row(
                "SELECT COALESCE(SUM(size_bytes),0) FROM attachments WHERE owner_id=?",
                params![owner],
                |row| row.get(0),
            )?;
            Ok(value.max(0) as u64)
        })
    }

    pub fn global_attachment_bytes(&self) -> Result<u64> {
        self.with_conn(|connection| {
            let value: i64 = connection.query_row(
                "SELECT COALESCE(SUM(size_bytes),0) FROM attachments",
                [],
                |row| row.get(0),
            )?;
            Ok(value.max(0) as u64)
        })
    }

    pub fn all_attachment_paths(&self) -> Result<Vec<(String, String, String, String)>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT attachment_id,owner_id,session_id,local_path FROM attachments ORDER BY created_at",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn attachments_older_than(
        &self,
        owner: Option<&str>,
        cutoff: &str,
    ) -> Result<Vec<AttachmentRecord>> {
        self.with_conn(|connection| {
            let sql = if owner.is_some() {
                ATTACHMENT_SELECT.to_owned()
                    + " WHERE owner_id=? AND created_at<? ORDER BY created_at"
            } else {
                ATTACHMENT_SELECT.to_owned() + " WHERE created_at<? ORDER BY created_at"
            };
            let mut statement = connection.prepare(&sql)?;
            let rows = if let Some(owner) = owner {
                statement
                    .query_map(params![owner, cutoff], row_attachment)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                statement
                    .query_map(params![cutoff], row_attachment)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(rows)
        })
    }

    pub fn session_has_active_run(&self, owner: &str, session_id: &str) -> Result<bool> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM agent_runs WHERE owner_principal=? AND session_id=? AND status IN ('received','context_build','running','awaiting_approval','verifying'))",
                    params![owner, session_id],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
    }

    pub fn delete_attachment(&self, owner: &str, attachment_id: &str) -> Result<bool> {
        self.with_conn(|connection| {
            Ok(connection.execute(
                "DELETE FROM attachments WHERE owner_id=? AND attachment_id=?",
                params![owner, attachment_id],
            )? == 1)
        })
    }

    pub fn insert_attachment(&self, record: NewAttachmentRecord<'_>) -> Result<()> {
        if !matches!(record.kind, "image" | "document")
            || record.size_bytes > i64::MAX as u64
            || record.original_name.trim().is_empty()
            || record.sha256.len() != 64
        {
            return Err(anyhow::anyhow!("invalid attachment metadata"));
        }
        self.with_conn(|connection| {
            let owns: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=?)",
                params![record.session_id, record.owner_id],
                |row| row.get(0),
            )?;
            if !owns {
                return Err(anyhow::anyhow!("attachment session does not belong to owner"));
            }
            let now = Utc::now().to_rfc3339();
            connection.execute(
                "INSERT INTO attachments(attachment_id,owner_id,session_id,telegram_file_id,telegram_unique_id,original_name,declared_mime,detected_mime,kind,size_bytes,sha256,local_path,processing_status,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?, 'downloaded',?,?)",
                params![record.attachment_id, record.owner_id, record.session_id, record.telegram_file_id, record.telegram_unique_id, record.original_name, record.declared_mime, record.detected_mime, record.kind, record.size_bytes as i64, record.sha256, record.local_path, now, now],
            )?;
            Ok(())
        })
    }

    pub fn set_attachment_status(
        &self,
        owner: &str,
        attachment_id: &str,
        status: &str,
        summary: Option<&str>,
        error: Option<&str>,
    ) -> Result<()> {
        if !matches!(
            status,
            "downloaded" | "processing" | "ready" | "needs_ocr" | "rejected" | "failed"
        ) {
            return Err(anyhow::anyhow!("invalid attachment status"));
        }
        self.with_conn(|connection| {
            let changed = connection.execute(
                "UPDATE attachments SET processing_status=?,summary=?,error=?,updated_at=? WHERE attachment_id=? AND owner_id=?",
                params![status, summary, error, Utc::now().to_rfc3339(), attachment_id, owner],
            )?;
            if changed != 1 {
                return Err(anyhow::anyhow!("attachment not found for owner"));
            }
            Ok(())
        })
    }

    pub fn replace_attachment_chunks(
        &self,
        owner: &str,
        attachment_id: &str,
        chunks: &[AttachmentChunkRecord],
    ) -> Result<()> {
        if chunks.len() > 10_000 {
            return Err(anyhow::anyhow!("attachment has too many chunks"));
        }
        self.with_conn(|connection| {
            let transaction = connection.transaction()?;
            let owns: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM attachments WHERE attachment_id=? AND owner_id=?)",
                params![attachment_id, owner],
                |row| row.get(0),
            )?;
            if !owns {
                return Err(anyhow::anyhow!("attachment not found for owner"));
            }
            transaction.execute(
                "DELETE FROM attachment_chunks WHERE attachment_id=?",
                params![attachment_id],
            )?;
            for (index, chunk) in chunks.iter().enumerate() {
                if chunk.attachment_id != attachment_id
                    || chunk.chunk_no != index
                    || chunk.text.trim().is_empty()
                    || chunk.text.chars().count() > 32_768
                {
                    return Err(anyhow::anyhow!("invalid attachment chunk"));
                }
                transaction.execute(
                    "INSERT INTO attachment_chunks(attachment_id,chunk_no,page_no,start_offset,end_offset,text) VALUES(?,?,?,?,?,?)",
                    params![chunk.attachment_id, chunk.chunk_no as i64, chunk.page_no.map(|value| value as i64), chunk.start_offset.map(|value| value as i64), chunk.end_offset.map(|value| value as i64), chunk.text],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn attachment(&self, owner: &str, attachment_id: &str) -> Result<Option<AttachmentRecord>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    &(ATTACHMENT_SELECT.to_owned() + " WHERE owner_id=? AND attachment_id=?"),
                    params![owner, attachment_id],
                    row_attachment,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn attachment_by_telegram_unique(
        &self,
        owner: &str,
        session_id: &str,
        telegram_unique_id: &str,
    ) -> Result<Option<AttachmentRecord>> {
        self.with_conn(|connection| {
            connection
                .query_row(
                    &(ATTACHMENT_SELECT.to_owned()
                        + " WHERE owner_id=? AND session_id=? AND telegram_unique_id=?"),
                    params![owner, session_id, telegram_unique_id],
                    row_attachment,
                )
                .optional()
                .map_err(Into::into)
        })
    }

    pub fn list_attachments(
        &self,
        owner: &str,
        session_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<AttachmentRecord>> {
        self.with_conn(|connection| {
            let (sql, bind_session) = if session_id.is_some() {
                (
                    ATTACHMENT_SELECT.to_owned()
                        + " WHERE owner_id=? AND session_id=? ORDER BY created_at DESC LIMIT ?",
                    true,
                )
            } else {
                (
                    ATTACHMENT_SELECT.to_owned()
                        + " WHERE owner_id=? ORDER BY created_at DESC LIMIT ?",
                    false,
                )
            };
            let mut statement = connection.prepare(&sql)?;
            let rows = if bind_session {
                statement
                    .query_map(
                        params![
                            owner,
                            session_id.unwrap_or_default(),
                            limit.clamp(1, 500) as i64
                        ],
                        row_attachment,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                statement
                    .query_map(params![owner, limit.clamp(1, 500) as i64], row_attachment)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(rows)
        })
    }

    pub fn recent_attachments(
        &self,
        owner: &str,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<AttachmentRecord>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                &(ATTACHMENT_SELECT.to_owned()
                    + " WHERE owner_id=? AND session_id=? ORDER BY created_at DESC LIMIT ?"),
            )?;
            let rows = statement.query_map(
                params![owner, session_id, limit.clamp(1, 100) as i64],
                row_attachment,
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn attachment_chunks(
        &self,
        owner: &str,
        attachment_id: &str,
        limit: usize,
    ) -> Result<Vec<AttachmentChunkRecord>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.attachment_id,c.chunk_no,c.page_no,c.start_offset,c.end_offset,c.text FROM attachment_chunks c JOIN attachments a ON a.attachment_id=c.attachment_id WHERE a.owner_id=? AND c.attachment_id=? ORDER BY c.chunk_no LIMIT ?",
            )?;
            let rows = statement.query_map(
                params![owner, attachment_id, limit.clamp(1, 10_000) as i64],
                row_attachment_chunk,
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    pub fn search_attachment_chunks(
        &self,
        owner: &str,
        session_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<AttachmentChunkRecord>> {
        let Some(query) = crate::memory::fts_query(query) else {
            return Ok(Vec::new());
        };
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.attachment_id,c.chunk_no,c.page_no,c.start_offset,c.end_offset,c.text FROM attachment_fts f JOIN attachment_chunks c ON c.id=f.rowid WHERE attachment_fts MATCH ? AND f.owner_id=? AND f.session_id=? ORDER BY bm25(attachment_fts),c.chunk_no LIMIT ?",
            )?;
            let rows = statement.query_map(
                params![query, owner, session_id, limit.clamp(1, 20) as i64],
                row_attachment_chunk,
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
    pub fn put_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| { conn.execute("INSERT INTO kv_settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value])?; Ok(()) })
    }
    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT value FROM kv_settings WHERE key=?",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }
    pub fn audit(&self, principal: &str, action: &str, detail: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO audit_events(principal,action,detail,created_at) VALUES(?,?,?,?)",
                params![principal, action, detail, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn audit_events(&self, principal: &str, limit: usize) -> Result<Vec<AuditEventRecord>> {
        self.with_conn(|connection| {
            let mut statement = connection.prepare(
                "SELECT id,principal,action,detail,created_at FROM audit_events WHERE principal=? ORDER BY id DESC LIMIT ?",
            )?;
            let rows = statement.query_map(
                params![principal, limit.clamp(1, 500) as i64],
                |row| {
                    Ok(AuditEventRecord {
                        id: row.get(0)?,
                        principal: row.get(1)?,
                        action: row.get(2)?,
                        detail: row.get(3)?,
                        created_at: row.get(4)?,
                    })
                },
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
    pub fn put_telegram_state(&self, key: &str, value: &str) -> Result<()> {
        self.with_conn(|conn| { conn.execute("INSERT INTO telegram_state(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key,value])?; Ok(()) })
    }
    pub fn telegram_state(&self, key: &str) -> Result<Option<String>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT value FROM telegram_state WHERE key=?",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Durable Telegram acceptance point: local inbox persistence and offset advance
    /// are committed atomically before asynchronous processing begins.
    pub fn enqueue_telegram_update(&self, update_id: i64, payload_json: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO telegram_inbox(update_id,payload_json,status,attempts,received_at) VALUES(?,?,'pending',0,?)",
                params![update_id,payload_json,Utc::now().to_rfc3339()]
            )? == 1;
            tx.execute(
                "INSERT INTO telegram_state(key,value) VALUES('offset',?) ON CONFLICT(key) DO UPDATE SET value=CASE WHEN CAST(excluded.value AS INTEGER)>CAST(value AS INTEGER) THEN excluded.value ELSE value END",
                params![(update_id+1).to_string()]
            )?;
            tx.commit()?;
            Ok(inserted)
        })
    }
    /// Processing is an uncertainty boundary: a command may have committed its semantic
    /// mutation immediately before the daemon crashed. Replaying it automatically could
    /// duplicate destructive effects, so interrupted work is quarantined for visibility
    /// instead of being re-enqueued. Only updates that were accepted but never claimed
    /// (`pending`) are automatically resumed on startup.
    pub fn quarantine_telegram_processing(&self) -> Result<usize> {
        self.with_conn(|conn| Ok(conn.execute("UPDATE telegram_inbox SET status='interrupted',last_error='daemon stopped while update was processing' WHERE status='processing'", [])?))
    }
    pub fn pending_telegram_updates(&self, limit: usize) -> Result<Vec<TelegramInboxRecord>> {
        self.with_conn(|conn| {
            let mut stmt=conn.prepare("SELECT update_id,payload_json,status,attempts FROM telegram_inbox WHERE status='pending' ORDER BY update_id LIMIT ?")?;
            let rows = stmt
                .query_map(params![limit as i64], |r| {
                    Ok(TelegramInboxRecord {
                        update_id: r.get(0)?,
                        payload_json: r.get(1)?,
                        status: r.get(2)?,
                        attempts: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }
    pub fn mark_telegram_processing(&self, update_id: i64) -> Result<bool> {
        self.with_conn(|conn| Ok(conn.execute("UPDATE telegram_inbox SET status='processing',attempts=attempts+1,last_error=NULL WHERE update_id=? AND status='pending'",params![update_id])?==1))
    }
    pub fn mark_telegram_processed(&self, update_id: i64) -> Result<()> {
        self.with_conn(|conn| {conn.execute("UPDATE telegram_inbox SET status='processed',processed_at=?,last_error=NULL WHERE update_id=?",params![Utc::now().to_rfc3339(),update_id])?;Ok(())})
    }
    /// Replace a recognized credential-input payload with a minimal audit
    /// marker. Processing updates are never replayed after a crash, so Xiao
    /// does not need to retain the owner's Telegram API-key message here.
    pub fn scrub_telegram_update_payload(&self, update_id: i64) -> Result<()> {
        let replacement = serde_json::json!({
            "update_id": update_id,
            "sensitive_input": "redacted"
        })
        .to_string();
        self.with_conn(|connection| {
            connection.execute(
                "UPDATE telegram_inbox SET payload_json=? WHERE update_id=? AND status IN ('pending','processing')",
                params![replacement, update_id],
            )?;
            Ok(())
        })
    }
    /// Handler failures are quarantined for the same reason as interrupted work: the
    /// handler can fail after a durable semantic mutation but before its Telegram reply.
    /// Retrying automatically would turn an at-most-once command execution policy into
    /// a potentially destructive duplicate execution policy.
    pub fn mark_telegram_failed(&self, update_id: i64, error: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE telegram_inbox SET status='failed',last_error=? WHERE update_id=?",
                params![error, update_id],
            )?;
            Ok(())
        })
    }
    pub fn telegram_inbox_problem_count(&self) -> Result<usize> {
        self.with_conn(|conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM telegram_inbox WHERE status IN ('failed','interrupted')",
                [],
                |r| r.get::<_, i64>(0),
            )? as usize)
        })
    }
    pub fn telegram_update_status(&self, update_id: i64) -> Result<Option<(String, i64)>> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT status,attempts FROM telegram_inbox WHERE update_id=?",
                params![update_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(Into::into)
        })
    }
}

fn row_session(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: r.get(0)?,
        owner_principal: r.get(1)?,
        name: r.get(2)?,
        provider: r.get(3)?,
        account_id: r.get(4)?,
        model: r.get(5)?,
        message_count: r.get(6)?,
        archived: r.get::<_, i64>(7)? != 0,
        is_side: r.get::<_, i64>(8)? != 0,
        parent_id: r.get(9)?,
        yolo_mode: r.get::<_, i64>(10)? != 0,
        created_at: r.get(11)?,
        last_active_at: r.get(12)?,
    })
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|existing| existing == column) {
        connection.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration};"
        ))?;
    }
    Ok(())
}

fn telegram_scope_from_principal(principal: &str) -> Option<(i64, i64)> {
    let parts = principal.split(':').collect::<Vec<_>>();
    match parts.as_slice() {
        ["telegram", chat, _owner] => Some((chat.parse().ok()?, 0)),
        ["telegram", chat, "topic", thread, _owner] => {
            Some((chat.parse().ok()?, thread.parse().ok()?))
        }
        _ => None,
    }
}

const ATTACHMENT_SELECT: &str = "SELECT attachment_id,owner_id,session_id,telegram_file_id,telegram_unique_id,original_name,declared_mime,detected_mime,kind,size_bytes,sha256,local_path,processing_status,summary,error,created_at,updated_at FROM attachments";

fn row_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRecord> {
    Ok(AttachmentRecord {
        attachment_id: row.get(0)?,
        owner_id: row.get(1)?,
        session_id: row.get(2)?,
        telegram_file_id: row.get(3)?,
        telegram_unique_id: row.get(4)?,
        original_name: row.get(5)?,
        declared_mime: row.get(6)?,
        detected_mime: row.get(7)?,
        kind: row.get(8)?,
        size_bytes: row.get::<_, i64>(9)?.max(0) as u64,
        sha256: row.get(10)?,
        local_path: row.get(11)?,
        processing_status: row.get(12)?,
        summary: row.get(13)?,
        error: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

fn row_attachment_chunk(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentChunkRecord> {
    Ok(AttachmentChunkRecord {
        attachment_id: row.get(0)?,
        chunk_no: row.get::<_, i64>(1)?.max(0) as usize,
        page_no: row
            .get::<_, Option<i64>>(2)?
            .map(|value| value.max(0) as usize),
        start_offset: row
            .get::<_, Option<i64>>(3)?
            .map(|value| value.max(0) as usize),
        end_offset: row
            .get::<_, Option<i64>>(4)?
            .map(|value| value.max(0) as usize),
        text: row.get(5)?,
    })
}

const APPROVAL_SELECT: &str = "SELECT id,owner_principal,session_id,agent_run_id,tool_call_id,capability,tool_name,arguments_hash,risk,summary,status,approval_mode,requested_at,decided_at,expires_at,consumed_at FROM approvals";

fn row_approval(row: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalRecord> {
    Ok(ApprovalRecord {
        id: row.get(0)?,
        owner_principal: row.get(1)?,
        session_id: row.get(2)?,
        agent_run_id: row.get(3)?,
        tool_call_id: row.get(4)?,
        capability: row.get(5)?,
        tool_name: row.get(6)?,
        arguments_hash: row.get(7)?,
        risk: row.get(8)?,
        summary: row.get(9)?,
        status: row.get(10)?,
        approval_mode: row.get(11)?,
        requested_at: row.get(12)?,
        decided_at: row.get(13)?,
        expires_at: row.get(14)?,
        consumed_at: row.get(15)?,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::context::SessionHistoryStore;
    #[test]
    fn persists_session_and_messages() {
        let db = Storage::open_memory().unwrap();
        let s = db
            .create_session("a", "one", "custom", None, "m", false, None)
            .unwrap();
        db.append_message("a", &s.id, "user", "hello").unwrap();
        assert_eq!(db.messages("a", &s.id).unwrap().len(), 1);
        assert_eq!(db.session("a", &s.id).unwrap().unwrap().message_count, 1);
    }
    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("xiao.db");
        let id = {
            let db = Storage::open(&path).unwrap();
            let session = db
                .create_session("a", "persistent", "custom", None, "m", false, None)
                .unwrap();
            db.append_message("a", &session.id, "user", "survives restart")
                .unwrap();
            session.id
        };
        let reopened = Storage::open(&path).unwrap();
        assert_eq!(
            reopened.messages("a", &id).unwrap()[0].content,
            "survives restart"
        );
        assert!(reopened.session("b", &id).unwrap().is_none());
        assert!(reopened.messages("b", &id).unwrap().is_empty());
    }
    #[test]
    fn cross_principal_operations_are_rejected() {
        let db = Storage::open_memory().unwrap();
        let b = db
            .create_session("b", "private", "custom", None, "m", false, None)
            .unwrap();
        db.append_message("b", &b.id, "user", "secret").unwrap();
        assert!(db.session("a", &b.id).unwrap().is_none());
        assert!(db.rename_session("a", &b.id, "stolen").is_err());
        assert!(db.archive_session("a", &b.id).is_err());
        assert!(db.messages("a", &b.id).unwrap().is_empty());
    }

    #[test]
    fn provider_default_reconciliation_preserves_an_explicit_valid_model() {
        let db = Storage::open_memory().unwrap();
        let inherited = db
            .create_session("a", "inherited", "custom", None, "old", false, None)
            .unwrap();
        let placeholder = db
            .create_session("b", "placeholder", "custom", None, "default", false, None)
            .unwrap();
        let explicit = db
            .create_session("c", "explicit", "custom", None, "kept", false, None)
            .unwrap();
        let invalid = db
            .create_session("d", "invalid", "custom", None, "missing", false, None)
            .unwrap();
        let other = db
            .create_session("e", "other", "codex", None, "old", false, None)
            .unwrap();

        let changed = db
            .reconcile_provider_models("custom", Some("old"), "new", &["new".into(), "kept".into()])
            .unwrap();
        assert_eq!(changed, 3);
        assert_eq!(
            db.session("a", &inherited.id).unwrap().unwrap().model,
            "new"
        );
        assert_eq!(
            db.session("b", &placeholder.id).unwrap().unwrap().model,
            "new"
        );
        assert_eq!(
            db.session("c", &explicit.id).unwrap().unwrap().model,
            "kept"
        );
        assert_eq!(db.session("d", &invalid.id).unwrap().unwrap().model, "new");
        assert_eq!(db.session("e", &other.id).unwrap().unwrap().model, "old");
    }

    #[test]
    fn durable_inbox_advances_only_with_persisted_update() {
        let db = Storage::open_memory().unwrap();
        assert!(db.enqueue_telegram_update(7, "{\"update_id\":7}").unwrap());
        assert_eq!(db.telegram_state("offset").unwrap().as_deref(), Some("8"));
        assert!(db.mark_telegram_processing(7).unwrap());
        assert!(db.pending_telegram_updates(10).unwrap().is_empty());
        assert_eq!(
            db.telegram_update_status(7).unwrap(),
            Some(("processing".into(), 1))
        );
        db.mark_telegram_processed(7).unwrap();
        assert!(db.pending_telegram_updates(10).unwrap().is_empty());
        assert_eq!(
            db.telegram_update_status(7).unwrap(),
            Some(("processed".into(), 1))
        );
        assert!(!db.enqueue_telegram_update(7, "duplicate").unwrap());
    }

    #[test]
    fn credential_input_payload_is_scrubbed_without_changing_inbox_state() {
        let db = Storage::open_memory().unwrap();
        let secret = "TELEGRAM_API_KEY_SENTINEL";
        assert!(db
            .enqueue_telegram_update(
                8,
                &format!(r#"{{"update_id":8,"message":{{"text":"{secret}"}}}}"#),
            )
            .unwrap());
        assert!(db.mark_telegram_processing(8).unwrap());
        db.scrub_telegram_update_payload(8).unwrap();
        let payload = db
            .with_conn(|connection| {
                connection
                    .query_row(
                        "SELECT payload_json FROM telegram_inbox WHERE update_id=8",
                        [],
                        |row| row.get::<_, String>(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap();
        assert!(!payload.contains(secret));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload).unwrap()["sensitive_input"],
            "redacted"
        );
        assert_eq!(
            db.telegram_update_status(8).unwrap(),
            Some(("processing".into(), 1))
        );
    }

    #[test]
    fn durable_inbox_quarantines_crash_during_processing_without_advancing_or_replaying() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inbox.db");
        {
            let db = Storage::open(&path).unwrap();
            assert!(db
                .enqueue_telegram_update(41, "{\"update_id\":41}")
                .unwrap());
            assert!(db.mark_telegram_processing(41).unwrap());
            assert_eq!(db.telegram_state("offset").unwrap().as_deref(), Some("42"));
        }
        let reopened = Storage::open(&path).unwrap();
        assert_eq!(reopened.quarantine_telegram_processing().unwrap(), 1);
        assert!(reopened.pending_telegram_updates(10).unwrap().is_empty());
        assert_eq!(
            reopened.telegram_update_status(41).unwrap(),
            Some(("interrupted".into(), 1))
        );
        assert_eq!(reopened.telegram_inbox_problem_count().unwrap(), 1);
        assert_eq!(
            reopened.telegram_state("offset").unwrap().as_deref(),
            Some("42")
        );
        assert!(!reopened.mark_telegram_processing(41).unwrap());
    }

    #[test]
    fn failed_processing_is_quarantined_and_duplicate_accept_is_idempotent() {
        let db = Storage::open_memory().unwrap();
        assert!(db.enqueue_telegram_update(9, "{\"update_id\":9}").unwrap());
        assert!(db.mark_telegram_processing(9).unwrap());
        db.mark_telegram_failed(9, "synthetic failure").unwrap();
        assert!(db.pending_telegram_updates(10).unwrap().is_empty());
        assert_eq!(
            db.telegram_update_status(9).unwrap(),
            Some(("failed".into(), 1))
        );
        assert!(!db
            .enqueue_telegram_update(9, "{\"update_id\":9,\"duplicate\":true}")
            .unwrap());
        assert_eq!(db.telegram_state("offset").unwrap().as_deref(), Some("10"));
    }

    #[test]
    fn accepted_but_unclaimed_update_replays_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending.db");
        {
            let db = Storage::open(&path).unwrap();
            assert!(db
                .enqueue_telegram_update(11, "{\"update_id\":11}")
                .unwrap());
            assert_eq!(db.telegram_state("offset").unwrap().as_deref(), Some("12"));
        }
        let reopened = Storage::open(&path).unwrap();
        assert_eq!(reopened.quarantine_telegram_processing().unwrap(), 0);
        let pending = reopened.pending_telegram_updates(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].update_id, 11);
        assert_eq!(pending[0].attempts, 0);
    }

    #[test]
    fn v020_migration_is_fresh_and_idempotent_with_consistent_fts() {
        let db = Storage::open_memory().unwrap();
        let session = db
            .create_session("p", "fresh", "custom", None, "m", false, None)
            .unwrap();
        db.append_message("p", &session.id, "user", "migration sentinel")
            .unwrap();
        db.migrate().unwrap();
        db.migrate().unwrap();
        db.with_conn(|connection| {
            let latest: i64 =
                connection.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get(0)
                })?;
            assert_eq!(latest, 17);
            for table in [
                "agent_runs",
                "tool_runs",
                "memories",
                "memory_history",
                "messages_fts",
                "session_summaries",
                "skills",
                "skill_history",
                "skills_fts",
                "approvals",
                "dependency_installs",
                "environment_probes",
                "workspace_file_index",
                "skill_file_index",
                "telegram_session_scopes",
                "telegram_active_sessions",
                "provider_capabilities",
                "owners",
                "legacy_owner_principals",
                "provider_profiles",
                "provider_profile_models",
                "attachments",
                "attachment_chunks",
                "attachment_fts",
            ] {
                let exists: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name=?)",
                    params![table],
                    |row| row.get(0),
                )?;
                assert!(exists, "missing migration object {table}");
            }
            let indexed: i64 = connection.query_row(
                "SELECT COUNT(*) FROM messages_fts WHERE messages_fts MATCH 'migration'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(indexed, 1);
            for (table, column) in [
                ("sessions", "yolo_mode"),
                ("skills", "prerequisites"),
                ("skills", "source_kind"),
                ("skills", "enabled"),
                ("tool_runs", "approval_mode"),
                ("tool_runs", "policy_original"),
                ("dependency_installs", "source"),
                ("dependency_installs", "validated"),
                ("provider_profile_models", "native_tools_state"),
                ("provider_profile_models", "structured_output_state"),
                ("provider_profile_models", "continuation_state"),
                ("provider_profile_models", "vision_state"),
                ("provider_profile_models", "file_input_state"),
            ] {
                let present: bool = connection.query_row(
                    &format!(
                        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('{table}') WHERE name=?)"
                    ),
                    params![column],
                    |row| row.get(0),
                )?;
                assert!(present, "missing {table}.{column}");
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn v010_database_upgrades_additively_without_losing_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.db");
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    r#"
                    CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY);
                    INSERT INTO schema_migrations(version) VALUES(1),(2),(3);
                    CREATE TABLE sessions(
                      id TEXT PRIMARY KEY,name TEXT NOT NULL,provider TEXT NOT NULL DEFAULT 'custom',
                      account_id TEXT,model TEXT NOT NULL DEFAULT 'default',archived INTEGER NOT NULL DEFAULT 0,
                      is_side INTEGER NOT NULL DEFAULT 0,parent_id TEXT,created_at TEXT NOT NULL,last_active_at TEXT NOT NULL
                    );
                    CREATE TABLE messages(
                      id INTEGER PRIMARY KEY AUTOINCREMENT,session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                      role TEXT NOT NULL,content TEXT NOT NULL,created_at TEXT NOT NULL
                    );
                    CREATE TABLE frontend_state(
                      principal TEXT PRIMARY KEY,active_main_session_id TEXT NOT NULL,
                      side_session_id TEXT,mode TEXT NOT NULL DEFAULT 'main'
                    );
                    INSERT INTO sessions(id,name,provider,model,created_at,last_active_at)
                      VALUES('legacy-session','Legacy','custom','m','now','now');
                    INSERT INTO messages(session_id,role,content,created_at)
                      VALUES('legacy-session','user','upgrade sentinel survives','now');
                    INSERT INTO frontend_state(principal,active_main_session_id,mode)
                      VALUES('legacy:owner','legacy-session','main');
                    "#,
                )
                .unwrap();
        }
        let upgraded = Arc::new(Storage::open(&path).unwrap());
        let session = upgraded
            .session("legacy:owner", "legacy-session")
            .unwrap()
            .unwrap();
        assert_eq!(session.owner_principal, "legacy:owner");
        assert_eq!(
            upgraded
                .messages("legacy:owner", &session.id)
                .unwrap()
                .len(),
            1
        );
        let hits = SessionHistoryStore::new(upgraded.clone())
            .search("legacy:owner", "upgrade sentinel", 10)
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert!(SessionHistoryStore::new(upgraded)
            .search("other", "upgrade sentinel", 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn reopen_quarantines_inflight_agent_and_tool_runs_without_replay() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runs.db");
        let (run_id, tool_id) = {
            let db = Storage::open(&path).unwrap();
            let session = db
                .create_session("p", "run", "custom", None, "m", false, None)
                .unwrap();
            let run = db
                .create_agent_run("p", &session.id, "custom", "m", Some("goal"))
                .unwrap();
            let tool = db
                .create_tool_run(&run, "call", "memory_set", "{}", "side_effect")
                .unwrap();
            db.set_tool_run_status(&tool, "running", None, None)
                .unwrap();
            (run, tool)
        };
        let reopened = Storage::open(&path).unwrap();
        assert_eq!(
            reopened.agent_run("p", &run_id).unwrap().unwrap().status,
            "interrupted"
        );
        let tools = reopened.tool_runs("p", &run_id).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, tool_id);
        assert_eq!(tools[0].status, "interrupted");
    }

    #[test]
    fn multiple_legacy_owner_rows_fail_closed_until_explicit_telegram_resolution() {
        let storage = Storage::open_memory().unwrap();
        storage
            .with_conn(|connection| {
                connection.execute(
                    "INSERT INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES('owner:telegram:41',41,'a','a')",
                    [],
                )?;
                connection.execute(
                    "INSERT INTO owners(owner_id,telegram_user_id,created_at,updated_at) VALUES('owner:telegram:42',42,'b','b')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let error = storage.management_owner_id().unwrap_err().to_string();
        assert!(error.contains("multiple legacy owners require explicit owner resolution"));

        let migration = storage.ensure_telegram_owner(42).unwrap();
        assert_eq!(migration.owner_id, "owner:telegram:42");
        assert_eq!(storage.management_owner_id().unwrap(), "owner:telegram:42");
        storage
            .with_conn(|connection| {
                let count: i64 =
                    connection.query_row("SELECT COUNT(*) FROM owners", [], |row| row.get(0))?;
                assert_eq!(count, 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn approval_is_exact_one_shot_and_cannot_cross_sessions_or_runs() {
        let storage = Storage::open_memory().unwrap();
        let session_a = storage
            .create_session("owner:test", "A", "custom", None, "m", false, None)
            .unwrap();
        let session_b = storage
            .create_session("owner:test", "B", "custom", None, "m", false, None)
            .unwrap();
        let run_a = storage
            .create_agent_run("owner:test", &session_a.id, "custom", "m", Some("A"))
            .unwrap();
        let run_b = storage
            .create_agent_run("owner:test", &session_b.id, "custom", "m", Some("B"))
            .unwrap();
        let a = ApprovalBinding {
            owner_id: "owner:test",
            session_id: &session_a.id,
            agent_run_id: &run_a,
            tool_call_id: "call-1",
            tool_name: "android_xiao_restart",
            arguments_hash: "same-arguments-hash",
        };
        let b = ApprovalBinding {
            owner_id: "owner:test",
            session_id: &session_b.id,
            agent_run_id: &run_b,
            tool_call_id: "call-1",
            tool_name: "android_xiao_restart",
            arguments_hash: "same-arguments-hash",
        };
        let request = storage
            .request_approval(ApprovalRequest {
                binding: a,
                capability: "android.service.restart",
                risk: "privileged",
                summary: "restart Xiao",
            })
            .unwrap();
        assert!(storage
            .decide_approval("owner:test", &request.id, true)
            .unwrap());
        assert!(!storage.consume_approval(b).unwrap());
        assert!(storage.consume_approval(a).unwrap());
        assert!(!storage.consume_approval(a).unwrap());
    }
}
