use crate::{
    config::Config,
    error::{Error, Result},
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    fs::{self, DirBuilder, File, OpenOptions},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};

pub fn private_dir(p: &Path) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(p)?;
    Ok(())
}
pub fn private_file(p: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(p)?)
}
pub struct Store {
    pub conn: Connection,
}
impl Store {
    pub fn open(c: &Config) -> Result<Self> {
        private_dir(&c.state_dir)?;
        let p = c.state_dir.join("spool.sqlite3");
        let _file = private_file(&p)?;
        let conn = Connection::open(p)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS messages(seq INTEGER PRIMARY KEY AUTOINCREMENT,app TEXT NOT NULL,id TEXT NOT NULL,chat TEXT NOT NULL,payload TEXT NOT NULL,delivered INTEGER NOT NULL DEFAULT 0,UNIQUE(app,id));
            CREATE INDEX IF NOT EXISTS pending ON messages(app,chat,delivered,seq);
            CREATE TABLE IF NOT EXISTS watermarks(app TEXT,chat TEXT,time INTEGER NOT NULL,PRIMARY KEY(app,chat));
            CREATE TABLE IF NOT EXISTS contexts(app TEXT,chat TEXT,id TEXT,kind TEXT,PRIMARY KEY(app,chat,id,kind)); COMMIT;")?;
        Ok(Self { conn })
    }
    pub fn insert(&self, v: &Value) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO messages(app,id,chat,payload) VALUES(?1,?2,?3,?4)",
            params![
                v["app_id"].as_str(),
                v["message_id"].as_str(),
                v["chat_id"].as_str(),
                v.to_string()
            ],
        )?;
        if let Some(id) = v["thread_id"].as_str() {
            self.context(
                v["app_id"].as_str().unwrap(),
                v["chat_id"].as_str().unwrap(),
                id,
                "thread",
            )?;
        }
        Ok(())
    }
    pub fn context(&self, app: &str, chat: &str, id: &str, kind: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO contexts(app,chat,id,kind) VALUES(?1,?2,?3,?4)",
            params![app, chat, id, kind],
        )?;
        Ok(())
    }
    pub fn contexts(&self, app: &str, chat: &str) -> Result<Vec<(String, String)>> {
        let mut q = self
            .conn
            .prepare("SELECT id,kind FROM contexts WHERE app=?1 AND chat=?2 ORDER BY id")?;
        let rows = q
            .query_map(params![app, chat], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    pub fn watermark(&self, app: &str, chat: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT time FROM watermarks WHERE app=?1 AND chat=?2",
                params![app, chat],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn recovery(
        &mut self,
        app: &str,
        chat: &str,
        end: Option<i64>,
        mut messages: Vec<Value>,
        threads: &[String],
        resolved_roots: &[String],
    ) -> Result<()> {
        messages.sort_by(|a, b| {
            let time = |v: &Value| v["create_time"].as_str().unwrap().parse::<i64>().unwrap();
            time(a)
                .cmp(&time(b))
                .then_with(|| a["message_id"].as_str().cmp(&b["message_id"].as_str()))
        });
        // Topological pass preserves time/id order amongst currently eligible nodes.
        let mut sorted = vec![];
        while !messages.is_empty() {
            let idx = messages
                .iter()
                .position(|v| !messages.iter().any(|p| p["message_id"] == v["reply_to"]))
                .ok_or_else(|| Error(1, "cyclic reply ancestry".into()))?;
            sorted.push(messages.remove(idx));
        }
        let tx = self.conn.unchecked_transaction()?;
        for v in sorted {
            self.insert(&v)?;
        }
        for id in threads {
            self.context(app, chat, id, "thread")?;
        }
        if let Some(end) = end {
            self.conn.execute("INSERT INTO watermarks(app,chat,time) VALUES(?1,?2,?3) ON CONFLICT(app,chat) DO UPDATE SET time=MAX(time,excluded.time)",params![app,chat,end])?;
        }
        for id in resolved_roots {
            self.conn.execute(
                "DELETE FROM contexts WHERE app=?1 AND chat=?2 AND id=?3 AND kind='root'",
                params![app, chat, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn pending(&self, app: &str) -> Result<Vec<(i64, Value)>> {
        let mut q = self.conn.prepare(
            "SELECT seq,payload FROM messages WHERE app=?1 AND delivered=0 ORDER BY seq",
        )?;
        let rows = q
            .query_map([app], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(seq, s)| {
                serde_json::from_str(&s)
                    .map(|v| (seq, v))
                    .map_err(|_| Error(4, "corrupt stored payload".into()))
            })
            .collect()
    }
    pub fn ack(&self, seq: i64) -> Result<()> {
        self.conn
            .execute("UPDATE messages SET delivered=1 WHERE seq=?1", [seq])?;
        Ok(())
    }
}

pub struct AppLock {
    _file: File,
}
impl AppLock {
    fn file(c: &Config) -> Result<File> {
        let root = crate::config::home()?.join("Library/Application Support/feishu-bridge/locks");
        private_dir(&root)?;
        private_file(&root.join(format!("{}.lock", c.app_id)))
    }
    pub fn acquire(c: &Config) -> Result<Self> {
        use std::os::fd::AsRawFd;
        let file = Self::file(c)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Error(5, "another listener holds the app lock".into()));
        }
        Ok(Self { _file: file })
    }
    pub fn running(c: &Config) -> Result<bool> {
        match Self::acquire(c) {
            Ok(_) => Ok(false),
            Err(Error(5, _)) => Ok(true),
            Err(e) => Err(e),
        }
    }
}
pub fn status(c: &Config) -> Result<Value> {
    let mut result = json!({"app_id":c.app_id,"running":AppLock::running(c)?,"pending":0,"delivered":0,"watermarks":{},"installed":crate::launchd::plist(c)?.exists()});
    let p = c.state_dir.join("spool.sqlite3");
    if fs::metadata(&p).is_ok() {
        let conn = Connection::open_with_flags(p, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let tables:i64=conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('messages','watermarks','contexts')",[],|r|r.get(0))?;
        // Opening a new spool creates the file before its atomic schema commit.
        // A status observer in that short interval sees a valid empty state.
        if tables == 0 {
            return Ok(result);
        }
        if tables != 3 {
            return Err(Error(4, "incomplete spool schema".into()));
        }
        for (key, flag) in [("pending", 0), ("delivered", 1)] {
            result[key] = json!(conn.query_row(
                "SELECT COUNT(*) FROM messages WHERE app=?1 AND delivered=?2",
                params![c.app_id, flag],
                |r| r.get::<_, i64>(0)
            )?);
        }
        let mut q = conn.prepare("SELECT chat,time FROM watermarks WHERE app=?1")?;
        for row in q.query_map([&c.app_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (chat, time) = row?;
            result["watermarks"][chat] = json!(time.to_string());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_sql_failure_rolls_back_messages_threads_and_watermark() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Config {
            app_id: "cli_test".into(),
            state_dir: tmp.path().join("state"),
            ..Config::default()
        };
        let mut store = Store::open(&c).unwrap();
        store.conn.execute_batch("CREATE TRIGGER reject_bad BEFORE INSERT ON messages WHEN new.id='om_bad' BEGIN SELECT RAISE(ABORT,'test fault'); END;").unwrap();
        let messages = vec![
            json!({"app_id":"cli_test","message_id":"om_good","chat_id":"oc_test","create_time":"1000","reply_to":null,"thread_id":"omt_test"}),
            json!({"app_id":"cli_test","message_id":"om_bad","chat_id":"oc_test","create_time":"2000","reply_to":null,"thread_id":null}),
        ];
        assert!(store
            .recovery(
                "cli_test",
                "oc_test",
                Some(3000),
                messages,
                &["omt_test".into()],
                &[]
            )
            .is_err());
        assert!(store.pending("cli_test").unwrap().is_empty());
        assert!(store.contexts("cli_test", "oc_test").unwrap().is_empty());
        assert_eq!(store.watermark("cli_test", "oc_test").unwrap(), None);
    }

    #[test]
    fn recovery_parents_precede_children_even_when_parent_clock_is_later() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Config {
            app_id: "cli_test".into(),
            state_dir: tmp.path().join("state"),
            ..Config::default()
        };
        let mut store = Store::open(&c).unwrap();
        let messages = vec![
            json!({"app_id":"cli_test","message_id":"om_child","chat_id":"oc_test","create_time":"1000","reply_to":"om_parent","thread_id":null}),
            json!({"app_id":"cli_test","message_id":"om_parent","chat_id":"oc_test","create_time":"2000","reply_to":null,"thread_id":null}),
        ];
        store
            .recovery("cli_test", "oc_test", Some(3000), messages, &[], &[])
            .unwrap();
        let ids = store
            .pending("cli_test")
            .unwrap()
            .iter()
            .map(|(_, v)| v["message_id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["om_parent", "om_child"]);
    }
    #[test]
    fn late_backfill_appends_behind_an_already_committed_live_message() {
        let tmp = tempfile::tempdir().unwrap();
        let c = Config {
            app_id: "cli_test".into(),
            state_dir: tmp.path().join("state"),
            ..Config::default()
        };
        let mut store = Store::open(&c).unwrap();
        store.insert(&json!({"app_id":"cli_test","message_id":"om_live","chat_id":"oc_test","create_time":"9000","thread_id":null})).unwrap();
        store.recovery("cli_test","oc_test",None,vec![json!({"app_id":"cli_test","message_id":"om_old","chat_id":"oc_test","create_time":"1000","reply_to":null,"thread_id":null})],&[],&[]).unwrap();
        let ids = store
            .pending("cli_test")
            .unwrap()
            .iter()
            .map(|(_, v)| v["message_id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["om_live", "om_old"]);
        assert_eq!(store.watermark("cli_test", "oc_test").unwrap(), None);
    }
}
