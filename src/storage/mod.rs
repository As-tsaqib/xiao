use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    pub created_at: String,
    pub last_active_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub role: String,
    pub content: String,
    pub created_at: String,
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
        Ok(s)
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let s = Self {
            conn: Mutex::new(conn),
        };
        s.migrate()?;
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
            Ok(())
        })
    }

    fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
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
    pub fn checkpoint(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
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
            "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.created_at,s.last_active_at FROM sessions s WHERE s.id=? AND s.owner_principal=?",
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
                "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.created_at,s.last_active_at FROM sessions s WHERE owner_principal=? AND is_side=0 ORDER BY last_active_at DESC LIMIT ? OFFSET ?"
            } else {
                "SELECT s.id,s.owner_principal,s.name,s.provider,s.account_id,s.model,(SELECT COUNT(*) FROM messages m WHERE m.session_id=s.id),s.archived,s.is_side,s.parent_id,s.created_at,s.last_active_at FROM sessions s WHERE owner_principal=? AND is_side=0 AND archived=0 ORDER BY last_active_at DESC LIMIT ? OFFSET ?"
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
                "SELECT EXISTS(SELECT 1 FROM provider_accounts WHERE id=? AND provider=? AND status='connected')",
                params![account_id,provider], |r| r.get(0)
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

    pub fn set_frontend_state(
        &self,
        principal: &str,
        main: &str,
        side: Option<&str>,
        mode: &str,
    ) -> Result<()> {
        self.with_conn(|conn| {
            let main_ok: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=? AND is_side=0 AND archived=0)",
                params![main,principal], |r| r.get(0)
            )?;
            if !main_ok { return Err(anyhow::anyhow!("main session is not owned by principal")); }
            if let Some(side_id) = side {
                let side_ok: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND owner_principal=? AND is_side=1 AND parent_id=?)",
                    params![side_id,principal,main], |r| r.get(0)
                )?;
                if !side_ok { return Err(anyhow::anyhow!("side session is not owned by principal/main")); }
            }
            conn.execute(
                "INSERT INTO frontend_state(principal,active_main_session_id,side_session_id,mode) VALUES(?,?,?,?) ON CONFLICT(principal) DO UPDATE SET active_main_session_id=excluded.active_main_session_id,side_session_id=excluded.side_session_id,mode=excluded.mode",
                params![principal,main,side,mode],
            )?;
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

    pub fn account(&self, id: &str) -> Result<Option<AccountRecord>> {
        self.with_conn(|conn| conn.query_row(
            "SELECT id,provider,label,email,status,access_expires_at,metadata_json FROM provider_accounts WHERE id=?",
            params![id],
            |r| Ok(AccountRecord{id:r.get(0)?,provider:r.get(1)?,label:r.get(2)?,email:r.get(3)?,status:r.get(4)?,access_expires_at:r.get(5)?,metadata_json:r.get(6)?})
        ).optional().map_err(Into::into))
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

    pub fn delete_account(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM provider_accounts WHERE id=?", params![id])?;
            Ok(())
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
        created_at: r.get(10)?,
        last_active_at: r.get(11)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
