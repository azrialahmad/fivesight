//! SQLite persistence: meetings, transcript segments, notes, AI notes, chat, and
//! full-text search over transcripts. One file on disk, no server.

use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("meeting {0} not found")]
    NotFound(i64),
}

pub type Result<T> = std::result::Result<T, StoreError>;

const SCHEMA: &str = "
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

CREATE TABLE IF NOT EXISTS meetings (
    id          INTEGER PRIMARY KEY,
    title       TEXT NOT NULL,
    started_at  INTEGER NOT NULL,
    ended_at    INTEGER,
    notes       TEXT NOT NULL DEFAULT '',
    enhanced    TEXT NOT NULL DEFAULT '',
    language    TEXT NOT NULL DEFAULT 'auto'
);

CREATE TABLE IF NOT EXISTS segments (
    id          INTEGER PRIMARY KEY,
    meeting_id  INTEGER NOT NULL REFERENCES meetings(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,
    seg_id      INTEGER NOT NULL,
    start_ms    INTEGER NOT NULL,
    text        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS segments_meeting ON segments(meeting_id, start_ms);

CREATE VIRTUAL TABLE IF NOT EXISTS segments_fts USING fts5(
    text, content='segments', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS segments_ai AFTER INSERT ON segments BEGIN
    INSERT INTO segments_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS segments_ad AFTER DELETE ON segments BEGIN
    INSERT INTO segments_fts(segments_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TABLE IF NOT EXISTS chat (
    id          INTEGER PRIMARY KEY,
    meeting_id  INTEGER REFERENCES meetings(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,
    content     TEXT NOT NULL,
    created_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS chat_meeting ON chat(meeting_id, id);
";

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Meeting {
    pub id: i64,
    pub title: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub notes: String,
    pub enhanced: String,
    /// "auto", "id" or "en": what the speech recognizer should expect.
    pub language: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSummary {
    pub id: i64,
    pub title: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub segment_count: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Segment {
    pub source: String,
    pub seg_id: i64,
    pub start_ms: i64,
    pub text: String,
    /// Participant name from the Meet extension, when known. Only set for other people.
    pub speaker: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub meeting_id: i64,
    pub meeting_title: String,
    pub start_ms: i64,
    /// Matching text with `[` `]` around hits.
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// Turn free text into a safe FTS5 query: each word quoted, joined with AND.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Create tables, then bring databases made by older versions up to date.
    fn init(conn: Connection) -> Result<Self> {
        // Order matters: columns added later must not be referenced by the base schema's
        // indexes, and `CREATE TABLE IF NOT EXISTS` leaves an old `meetings` table untouched.
        conn.execute_batch(SCHEMA)?;
        let has_language: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('meetings') WHERE name = 'language'",
            [],
            |r| r.get(0),
        )?;
        if has_language == 0 {
            conn.execute_batch("ALTER TABLE meetings ADD COLUMN language TEXT NOT NULL DEFAULT 'auto'")?;
        }
        let has_speaker: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('segments') WHERE name = 'speaker'",
            [],
            |r| r.get(0),
        )?;
        if has_speaker == 0 {
            conn.execute_batch("ALTER TABLE segments ADD COLUMN speaker TEXT")?;
        }
        Ok(Self { conn })
    }

    pub fn create_meeting(&self, title: &str, started_at: i64) -> Result<i64> {
        self.create_meeting_in(title, started_at, "auto")
    }

    pub fn create_meeting_in(&self, title: &str, started_at: i64, language: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO meetings (title, started_at, language) VALUES (?1, ?2, ?3)",
            params![title, started_at, language],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn set_language(&self, id: i64, language: &str) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET language = ?2 WHERE id = ?1", params![id, language])?;
        Ok(())
    }

    pub fn end_meeting(&self, id: i64, ended_at: i64) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET ended_at = ?2 WHERE id = ?1", params![id, ended_at])?;
        Ok(())
    }

    /// Mark a finished meeting as in progress again (recording into it a second time).
    pub fn reopen_meeting(&self, id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET ended_at = NULL WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn rename_meeting(&self, id: i64, title: &str) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET title = ?2 WHERE id = ?1", params![id, title])?;
        Ok(())
    }

    pub fn set_notes(&self, id: i64, notes: &str) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET notes = ?2 WHERE id = ?1", params![id, notes])?;
        Ok(())
    }

    pub fn set_enhanced(&self, id: i64, enhanced: &str) -> Result<()> {
        self.conn
            .execute("UPDATE meetings SET enhanced = ?2 WHERE id = ?1", params![id, enhanced])?;
        Ok(())
    }

    pub fn delete_meeting(&self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM meetings WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn get_meeting(&self, id: i64) -> Result<Meeting> {
        self.conn
            .query_row(
                "SELECT id, title, started_at, ended_at, notes, enhanced, language FROM meetings WHERE id = ?1",
                params![id],
                |r| {
                    Ok(Meeting {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                        notes: r.get(4)?,
                        enhanced: r.get(5)?,
                        language: r.get(6)?,
                    })
                },
            )
            .optional()?
            .ok_or(StoreError::NotFound(id))
    }

    /// Newest first.
    pub fn list_meetings(&self) -> Result<Vec<MeetingSummary>> {
        let mut st = self.conn.prepare(
            "SELECT m.id, m.title, m.started_at, m.ended_at,
                    (SELECT COUNT(*) FROM segments s WHERE s.meeting_id = m.id)
             FROM meetings m ORDER BY m.started_at DESC, m.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(MeetingSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                started_at: r.get(2)?,
                ended_at: r.get(3)?,
                segment_count: r.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn add_segment(&self, meeting_id: i64, source: &str, seg_id: i64, start_ms: i64, text: &str) -> Result<()> {
        self.add_segment_by(meeting_id, source, seg_id, start_ms, text, None)
    }

    /// Like add_segment, with the speaker's name when the Meet extension supplied one.
    pub fn add_segment_by(
        &self,
        meeting_id: i64,
        source: &str,
        seg_id: i64,
        start_ms: i64,
        text: &str,
        speaker: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO segments (meeting_id, source, seg_id, start_ms, text, speaker) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![meeting_id, source, seg_id, start_ms, text, speaker],
        )?;
        Ok(())
    }

    pub fn transcript(&self, meeting_id: i64) -> Result<Vec<Segment>> {
        let mut st = self.conn.prepare(
            "SELECT source, seg_id, start_ms, text, speaker FROM segments WHERE meeting_id = ?1 ORDER BY start_ms, id",
        )?;
        let rows = st.query_map(params![meeting_id], |r| {
            Ok(Segment { source: r.get(0)?, seg_id: r.get(1)?, start_ms: r.get(2)?, text: r.get(3)?, speaker: r.get(4)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Full-text search across every meeting's transcript.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let Some(q) = fts_query(query) else { return Ok(Vec::new()) };
        let mut st = self.conn.prepare(
            "SELECT s.meeting_id, m.title, s.start_ms, snippet(segments_fts, 0, '[', ']', '…', 12)
             FROM segments_fts
             JOIN segments s ON s.id = segments_fts.rowid
             JOIN meetings m ON m.id = s.meeting_id
             WHERE segments_fts MATCH ?1
             ORDER BY rank LIMIT ?2",
        )?;
        let rows = st.query_map(params![q, limit as i64], |r| {
            Ok(SearchHit { meeting_id: r.get(0)?, meeting_title: r.get(1)?, start_ms: r.get(2)?, snippet: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn add_chat(&self, meeting_id: Option<i64>, role: &str, content: &str, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO chat (meeting_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![meeting_id, role, content, now],
        )?;
        Ok(())
    }

    /// Chat for one meeting, or the global chat when `meeting_id` is `None`. Oldest first.
    pub fn chat_history(&self, meeting_id: Option<i64>) -> Result<Vec<ChatMessage>> {
        let mut st = self
            .conn
            .prepare("SELECT role, content FROM chat WHERE meeting_id IS ?1 ORDER BY id")?;
        let rows = st.query_map(params![meeting_id], |r| Ok(ChatMessage { role: r.get(0)?, content: r.get(1)? }))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_memory().unwrap()
    }

    #[test]
    fn meeting_lifecycle() {
        let s = store();
        let id = s.create_meeting("Standup", 1000).unwrap();
        s.set_notes(id, "- PAY-142 timeout").unwrap();
        s.set_enhanced(id, "## Notes\n...").unwrap();
        s.end_meeting(id, 5000).unwrap();
        s.rename_meeting(id, "Daily standup").unwrap();
        let m = s.get_meeting(id).unwrap();
        assert_eq!(m.title, "Daily standup");
        assert_eq!(m.ended_at, Some(5000));
        assert_eq!(m.notes, "- PAY-142 timeout");
        assert!(matches!(s.get_meeting(999), Err(StoreError::NotFound(999))));
        s.reopen_meeting(id).unwrap();
        assert_eq!(s.get_meeting(id).unwrap().ended_at, None);
    }

    #[test]
    fn transcript_is_time_ordered_and_counted() {
        let s = store();
        let id = s.create_meeting("m", 0).unwrap();
        s.add_segment(id, "mic", 2, 9000, "second").unwrap();
        s.add_segment(id, "loopback", 1, 1000, "first").unwrap();
        let t = s.transcript(id).unwrap();
        assert_eq!(t.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(), ["first", "second"]);
        assert_eq!(s.list_meetings().unwrap()[0].segment_count, 2);
    }

    #[test]
    fn speaker_names_are_stored_and_old_databases_gain_the_column() {
        let s = store();
        let id = s.create_meeting("m", 0).unwrap();
        s.add_segment_by(id, "loopback", 1, 1000, "hello", Some("Budi")).unwrap();
        s.add_segment(id, "mic", 2, 2000, "hi").unwrap();
        let t = s.transcript(id).unwrap();
        assert_eq!(t[0].speaker.as_deref(), Some("Budi"));
        assert_eq!(t[1].speaker, None);

        // A database from before the column existed.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE meetings (id INTEGER PRIMARY KEY, title TEXT NOT NULL, started_at INTEGER NOT NULL, ended_at INTEGER, notes TEXT NOT NULL DEFAULT '', enhanced TEXT NOT NULL DEFAULT '');
             CREATE TABLE segments (id INTEGER PRIMARY KEY, meeting_id INTEGER NOT NULL, source TEXT NOT NULL, seg_id INTEGER NOT NULL, start_ms INTEGER NOT NULL, text TEXT NOT NULL);
             INSERT INTO meetings (title, started_at) VALUES ('old', 1);
             INSERT INTO segments (meeting_id, source, seg_id, start_ms, text) VALUES (1, 'mic', 1, 0, 'before');",
        )
        .unwrap();
        let s = Store::init(conn).unwrap();
        let t = s.transcript(1).unwrap();
        assert_eq!(t[0].text, "before");
        assert_eq!(t[0].speaker, None);
        s.add_segment_by(1, "loopback", 2, 5, "after", Some("Sari")).unwrap();
        assert_eq!(s.transcript(1).unwrap()[1].speaker.as_deref(), Some("Sari"));
    }

    #[test]
    fn list_is_newest_first() {
        let s = store();
        s.create_meeting("old", 100).unwrap();
        s.create_meeting("new", 200).unwrap();
        let l = s.list_meetings().unwrap();
        assert_eq!(l[0].title, "new");
    }

    #[test]
    fn search_finds_across_meetings_with_snippet() {
        let s = store();
        let a = s.create_meeting("Payments sync", 1).unwrap();
        let b = s.create_meeting("Retro", 2).unwrap();
        s.add_segment(a, "loopback", 1, 4000, "the checkout timeout is thirty seconds").unwrap();
        s.add_segment(b, "mic", 1, 8000, "we should celebrate the release").unwrap();
        let hits = s.search("checkout timeout", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].meeting_id, a);
        assert!(hits[0].snippet.contains("[checkout]"), "{}", hits[0].snippet);
    }

    #[test]
    fn search_survives_hostile_input() {
        let s = store();
        let a = s.create_meeting("m", 1).unwrap();
        s.add_segment(a, "mic", 1, 0, "hello world").unwrap();
        for q in ["", "   ", "\"", "hello OR", "NEAR(", "a*b", "'; DROP TABLE meetings;--"] {
            assert!(s.search(q, 5).is_ok(), "query {q:?}");
        }
        assert_eq!(s.list_meetings().unwrap().len(), 1);
    }

    #[test]
    fn delete_cascades_and_cleans_index() {
        let s = store();
        let id = s.create_meeting("m", 1).unwrap();
        s.add_segment(id, "mic", 1, 0, "unique zebra").unwrap();
        s.add_chat(Some(id), "user", "hi", 1).unwrap();
        s.delete_meeting(id).unwrap();
        assert!(s.transcript(id).unwrap().is_empty());
        assert!(s.search("zebra", 5).unwrap().is_empty());
        assert!(s.chat_history(Some(id)).unwrap().is_empty());
    }

    #[test]
    fn chat_scoped_per_meeting_and_global() {
        let s = store();
        let id = s.create_meeting("m", 1).unwrap();
        s.add_chat(Some(id), "user", "q1", 1).unwrap();
        s.add_chat(Some(id), "assistant", "a1", 2).unwrap();
        s.add_chat(None, "user", "global", 3).unwrap();
        assert_eq!(s.chat_history(Some(id)).unwrap().len(), 2);
        assert_eq!(s.chat_history(None).unwrap(), vec![ChatMessage { role: "user".into(), content: "global".into() }]);
    }

    #[test]
    fn language_defaults_to_auto_and_can_change() {
        let s = store();
        let a = s.create_meeting("a", 1).unwrap();
        let b = s.create_meeting_in("b", 2, "id").unwrap();
        assert_eq!(s.get_meeting(a).unwrap().language, "auto");
        assert_eq!(s.get_meeting(b).unwrap().language, "id");
        s.set_language(a, "en").unwrap();
        assert_eq!(s.get_meeting(a).unwrap().language, "en");
    }

    #[test]
    fn migrates_a_database_from_before_the_language_column() {
        let dir = std::env::temp_dir().join(format!("fs_store_migrate_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        {
            // The schema exactly as the first release created it, with a meeting in it.
            let c = Connection::open(&path).unwrap();
            c.execute_batch(
                "CREATE TABLE meetings (id INTEGER PRIMARY KEY, title TEXT NOT NULL, started_at INTEGER NOT NULL,
                    ended_at INTEGER, notes TEXT NOT NULL DEFAULT '', enhanced TEXT NOT NULL DEFAULT '');
                 INSERT INTO meetings (title, started_at, notes) VALUES ('old meeting', 5, 'kept notes');",
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        let m = s.get_meeting(1).unwrap();
        assert_eq!((m.title.as_str(), m.notes.as_str(), m.language.as_str()), ("old meeting", "kept notes", "auto"));
        // Opening again must be a no-op, not an error.
        drop(s);
        assert!(Store::open(&path).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn persists_to_disk() {
        let dir = std::env::temp_dir().join(format!("fs_store_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.db");
        {
            let s = Store::open(&path).unwrap();
            s.create_meeting("kept", 1).unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.list_meetings().unwrap()[0].title, "kept");
        let _ = std::fs::remove_dir_all(dir);
    }
}
