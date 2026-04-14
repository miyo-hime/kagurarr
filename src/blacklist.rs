use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection};

pub struct Blacklist {
    conn: Connection,
}

#[derive(Debug)]
pub enum BlacklistStatus {
    Failed,
    Done,
    NoMatch,
}

impl BlacklistStatus {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::Done => "done",
            Self::NoMatch => "no_match",
        }
    }
}

impl Blacklist {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("couldn't open database at {path}"))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS blacklist (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                lidarr_album_id INTEGER NOT NULL,
                slskd_username  TEXT,
                remote_folder   TEXT,
                status          TEXT NOT NULL,
                reason          TEXT,
                attempted_at    TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_blacklist_album
                ON blacklist(lidarr_album_id, status);",
        )
        .context("failed to initialize database schema")?;

        Ok(Self { conn })
    }

    /// check if this (album, user, folder) combo is already blacklisted
    pub fn is_blacklisted(&self, album_id: i64, username: &str, folder: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM blacklist
             WHERE lidarr_album_id = ?1
               AND slskd_username  = ?2
               AND remote_folder   = ?3
               AND status = 'failed'",
            params![album_id, username, folder],
            |row| row.get(0),
        )
        .context("blacklist query failed")?;

        Ok(count > 0)
    }

    /// check if an album is already marked done
    pub fn is_done(&self, album_id: i64) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM blacklist
             WHERE lidarr_album_id = ?1 AND status = 'done'",
            params![album_id],
            |row| row.get(0),
        )
        .context("blacklist done-check failed")?;

        Ok(count > 0)
    }

    pub fn insert(
        &self,
        album_id: i64,
        username: Option<&str>,
        folder: Option<&str>,
        status: BlacklistStatus,
        reason: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO blacklist (lidarr_album_id, slskd_username, remote_folder, status, reason, attempted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                album_id,
                username,
                folder,
                status.as_str(),
                reason,
                Utc::now().to_rfc3339(),
            ],
        )
        .context("blacklist insert failed")?;

        Ok(())
    }

    /// clean up old failed entries past the TTL so we retry eventually
    pub fn prune_expired(&self, ttl_days: u64) -> Result<usize> {
        let cutoff = Utc::now()
            .checked_sub_signed(chrono::Duration::days(ttl_days as i64))
            .unwrap()
            .to_rfc3339();

        let deleted = self.conn.execute(
            "DELETE FROM blacklist WHERE status = 'failed' AND attempted_at < ?1",
            params![cutoff],
        )
        .context("blacklist prune failed")?;

        Ok(deleted)
    }
}
