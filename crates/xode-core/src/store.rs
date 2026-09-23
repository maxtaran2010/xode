use crate::types::{Message, Mode};
use anyhow::Result;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub root: String,
    pub extra_roots: Vec<String>,
    pub created_at: i64,
    pub last_used: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionInfo {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub goal: Option<String>,
    pub mode: Mode,
    pub model: String,
    pub gateway: String,
    pub segment: u32,
    /// Latest compaction state text (seed for the current segment).
    pub state: String,
    pub total_work_ms: u64,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub tool_calls: u32,
    pub compactions: u32,
    pub cwd: Option<String>,
    /// Knowledge-base layers/sources switched off for this chat.
    #[serde(default)]
    pub kb_off: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Segment {
    pub session_id: String,
    pub index: u32,
    pub path: String,
    pub state: String,
    pub tokens_before: u64,
    pub tokens_after: u64,
    pub created_at: i64,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS projects(
                id TEXT PRIMARY KEY, name TEXT NOT NULL, root TEXT NOT NULL UNIQUE,
                extra_roots TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL, last_used INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS sessions(
                id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_id, updated_at);
            CREATE TABLE IF NOT EXISTS messages(
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                seq INTEGER NOT NULL, data TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id, seq);
            CREATE TABLE IF NOT EXISTS segments(
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                idx INTEGER NOT NULL, data TEXT NOT NULL, PRIMARY KEY(session_id, idx));",
        )?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&crate::config::data_dir().join("xode.db"))
    }

    // ---------- projects
    pub fn projects(&self) -> Result<Vec<Project>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT id,name,root,extra_roots,created_at,last_used FROM projects ORDER BY last_used DESC")?;
        let rows = st.query_map([], |r| {
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                root: r.get(2)?,
                extra_roots: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
                created_at: r.get(4)?,
                last_used: r.get(5)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn project(&self, id: &str) -> Result<Option<Project>> {
        Ok(self.projects()?.into_iter().find(|p| p.id == id))
    }

    pub fn project_by_root(&self, root: &str) -> Result<Option<Project>> {
        let norm = normalize_root(root);
        Ok(self.projects()?.into_iter().find(|p| normalize_root(&p.root) == norm))
    }

    pub fn add_project(&self, root: &str, name: Option<&str>) -> Result<Project> {
        if let Some(p) = self.project_by_root(root)? {
            self.touch_project(&p.id)?;
            return Ok(p);
        }
        let name = name.map(|s| s.to_string()).unwrap_or_else(|| {
            Path::new(root).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| root.to_string())
        });
        let p = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            root: root.to_string(),
            extra_roots: vec![],
            created_at: now(),
            last_used: now(),
        };
        self.conn.lock().execute(
            "INSERT INTO projects(id,name,root,extra_roots,created_at,last_used) VALUES(?1,?2,?3,'[]',?4,?5)",
            params![p.id, p.name, p.root, p.created_at, p.last_used],
        )?;
        Ok(p)
    }

    pub fn update_project(&self, p: &Project) -> Result<()> {
        self.conn.lock().execute(
            "UPDATE projects SET name=?2, root=?3, extra_roots=?4 WHERE id=?1",
            params![p.id, p.name, p.root, serde_json::to_string(&p.extra_roots)?],
        )?;
        Ok(())
    }

    pub fn touch_project(&self, id: &str) -> Result<()> {
        self.conn.lock().execute("UPDATE projects SET last_used=?2 WHERE id=?1", params![id, now()])?;
        Ok(())
    }

    pub fn remove_project(&self, id: &str) -> Result<()> {
        self.conn.lock().execute("DELETE FROM projects WHERE id=?1", params![id])?;
        Ok(())
    }

    // ---------- sessions
    pub fn sessions(&self, project_id: Option<&str>) -> Result<Vec<SessionInfo>> {
        let c = self.conn.lock();
        let mut out = vec![];
        let mut push = |data: String| {
            if let Ok(s) = serde_json::from_str::<SessionInfo>(&data) {
                out.push(s);
            }
        };
        match project_id {
            Some(pid) => {
                let mut st = c.prepare("SELECT data FROM sessions WHERE project_id=?1 ORDER BY updated_at DESC")?;
                for r in st.query_map([pid], |r| r.get::<_, String>(0))? {
                    push(r?);
                }
            }
            None => {
                let mut st = c.prepare("SELECT data FROM sessions ORDER BY updated_at DESC")?;
                for r in st.query_map([], |r| r.get::<_, String>(0))? {
                    push(r?);
                }
            }
        }
        Ok(out)
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionInfo>> {
        let c = self.conn.lock();
        let d: Option<String> = c.query_row("SELECT data FROM sessions WHERE id=?1", [id], |r| r.get(0)).optional()?;
        Ok(d.and_then(|d| serde_json::from_str(&d).ok()))
    }

    pub fn create_session(&self, project_id: &str) -> Result<SessionInfo> {
        let s = SessionInfo {
            id: uuid::Uuid::new_v4().to_string(),
            project_id: project_id.into(),
            title: String::new(),
            created_at: now(),
            updated_at: now(),
            ..Default::default()
        };
        self.save_session(&s)?;
        Ok(s)
    }

    pub fn save_session(&self, s: &SessionInfo) -> Result<()> {
        self.conn.lock().execute(
            "INSERT INTO sessions(id,project_id,created_at,updated_at,data) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(id) DO UPDATE SET updated_at=excluded.updated_at, data=excluded.data, project_id=excluded.project_id",
            params![s.id, s.project_id, s.created_at, s.updated_at, serde_json::to_string(s)?],
        )?;
        Ok(())
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.conn.lock().execute("DELETE FROM sessions WHERE id=?1", params![id])?;
        Ok(())
    }

    // ---------- messages
    pub fn messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT data FROM messages WHERE session_id=?1 ORDER BY seq")?;
        let rows = st.query_map([session_id], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect())
    }

    pub fn append_message(&self, session_id: &str, m: &Message) -> Result<()> {
        let c = self.conn.lock();
        let seq: i64 =
            c.query_row("SELECT COALESCE(MAX(seq),0)+1 FROM messages WHERE session_id=?1", [session_id], |r| r.get(0))?;
        c.execute(
            "INSERT OR REPLACE INTO messages(id,session_id,seq,data) VALUES(?1,?2,?3,?4)",
            params![m.id, session_id, seq, serde_json::to_string(m)?],
        )?;
        Ok(())
    }

    pub fn update_message(&self, m: &Message) -> Result<()> {
        self.conn.lock().execute("UPDATE messages SET data=?2 WHERE id=?1", params![m.id, serde_json::to_string(m)?])?;
        Ok(())
    }

    /// Deletes `message_id` (when `inclusive`) and everything after it. Returns the count removed.
    pub fn truncate_messages(&self, session_id: &str, message_id: &str, inclusive: bool) -> Result<usize> {
        let c = self.conn.lock();
        let seq: i64 = c.query_row("SELECT seq FROM messages WHERE id=?1 AND session_id=?2", params![message_id, session_id], |r| r.get(0))?;
        let n = c.execute(
            if inclusive { "DELETE FROM messages WHERE session_id=?1 AND seq>=?2" } else { "DELETE FROM messages WHERE session_id=?1 AND seq>?2" },
            params![session_id, seq],
        )?;
        Ok(n)
    }

    /// Drops compaction segments newer than `max_index`.
    pub fn truncate_segments(&self, session_id: &str, max_index: u32) -> Result<()> {
        self.conn.lock().execute("DELETE FROM segments WHERE session_id=?1 AND idx>?2", params![session_id, max_index])?;
        Ok(())
    }

    pub fn clear_messages(&self, session_id: &str) -> Result<()> {
        let c = self.conn.lock();
        c.execute("DELETE FROM messages WHERE session_id=?1", [session_id])?;
        c.execute("DELETE FROM segments WHERE session_id=?1", [session_id])?;
        Ok(())
    }

    // ---------- segments
    pub fn add_segment(&self, s: &Segment) -> Result<()> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO segments(session_id,idx,data) VALUES(?1,?2,?3)",
            params![s.session_id, s.index, serde_json::to_string(s)?],
        )?;
        Ok(())
    }

    pub fn segments(&self, session_id: &str) -> Result<Vec<Segment>> {
        let c = self.conn.lock();
        let mut st = c.prepare("SELECT data FROM segments WHERE session_id=?1 ORDER BY idx")?;
        let rows = st.query_map([session_id], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|r| r.ok()).filter_map(|d| serde_json::from_str(&d).ok()).collect())
    }
}

fn normalize_root(r: &str) -> String {
    let s = r.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) {
        s.to_lowercase()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Message;

    #[test]
    fn truncate_messages_and_segments() {
        let dir = tempfile::tempdir().unwrap();
        let st = Store::open(&dir.path().join("db")).unwrap();
        let p = st.add_project(&dir.path().to_string_lossy(), None).unwrap();
        let s = st.create_session(&p.id).unwrap();
        let msgs: Vec<Message> = (0..5).map(|i| Message::user(format!("m{i}"))).collect();
        for m in &msgs {
            st.append_message(&s.id, m).unwrap();
        }
        assert_eq!(st.truncate_messages(&s.id, &msgs[3].id, false).unwrap(), 1);
        assert_eq!(st.truncate_messages(&s.id, &msgs[1].id, true).unwrap(), 3);
        let left: Vec<String> = st.messages(&s.id).unwrap().iter().map(|m| m.text()).collect();
        assert_eq!(left, vec!["m0"]);
        for i in 1..=3 {
            st.add_segment(&Segment {
                session_id: s.id.clone(),
                index: i,
                path: String::new(),
                state: String::new(),
                tokens_before: 0,
                tokens_after: 0,
                created_at: 0,
            })
            .unwrap();
        }
        st.truncate_segments(&s.id, 1).unwrap();
        assert_eq!(st.segments(&s.id).unwrap().len(), 1);
    }
}
