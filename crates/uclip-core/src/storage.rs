use std::{path::Path, time::Duration};

use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};

use crate::{AppError, ErrorCode, SettingsSnapshot, Theme, UpdateSettings};

pub const SCHEMA_VERSION: u32 = 2;
const APPLICATION_ID: i64 = 0x434c4e58; // CLNX: never open the previous application's database.

/// Owned by one dedicated worker. No Tauri handles or frontend state enter storage.
pub struct Storage {
    pub(crate) connection: Connection,
}

impl Storage {
    pub fn open(path: &Path) -> Result<Self, AppError> {
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        // Check ownership/version before enabling WAL or performing any migrations.
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application_id: i64 =
            tx.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let objects: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;
        if application_id != APPLICATION_ID && (application_id != 0 || version != 0 || objects != 0)
        {
            return Err(AppError::new(ErrorCode::ForeignDatabase));
        }
        if version > SCHEMA_VERSION {
            return Err(AppError::new(ErrorCode::UnsupportedSchema));
        }
        if version == 0 {
            tx.execute_batch(
                "CREATE TABLE settings (
                    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                    theme TEXT NOT NULL CHECK (theme IN ('system', 'light', 'dark')),
                    revision INTEGER NOT NULL CHECK (revision >= 0)
                ) STRICT;
                INSERT INTO settings(singleton, theme, revision) VALUES(1, 'system', 0);",
            )?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            tx.execute_batch("CREATE TABLE capture_policy(singleton INTEGER PRIMARY KEY CHECK(singleton=1),enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),exclude_pattern TEXT NOT NULL,max_items INTEGER NOT NULL CHECK(max_items BETWEEN 1 AND 1000),revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            INSERT INTO capture_policy VALUES(1,0,'',500,0);
            CREATE TABLE history_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            INSERT INTO history_meta VALUES(1,0);
            CREATE TABLE clips(id TEXT PRIMARY KEY,kind TEXT NOT NULL CHECK(kind IN ('text','image','files')),text TEXT NOT NULL,data BLOB NOT NULL,width INTEGER,height INTEGER,file_count INTEGER NOT NULL DEFAULT 0,fingerprint BLOB NOT NULL,origin TEXT NOT NULL DEFAULT 'capture',pinned INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,collection TEXT,captured_at INTEGER NOT NULL,last_used_at INTEGER,sequence INTEGER NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            CREATE INDEX clips_order ON clips(deleted,sequence DESC,id);
            CREATE INDEX clips_fingerprint ON clips(fingerprint);")?;
            tx.pragma_update(None, "user_version", 2)?;
        }
        tx.commit()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let storage = Self { connection };
        storage.capture_policy()?;
        storage.history_revision()?;
        storage.settings()?; // Fail startup on an invalid persisted setting.
        Ok(storage)
    }

    pub fn settings(&self) -> Result<SettingsSnapshot, AppError> {
        let (theme, revision): (String, i64) = self.connection.query_row(
            "SELECT theme, revision FROM settings WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let theme = match theme.as_str() {
            "system" => Theme::System,
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => return Err(AppError::new(ErrorCode::Storage)),
        };
        if revision < 0 {
            return Err(AppError::new(ErrorCode::Storage));
        }
        Ok(SettingsSnapshot {
            theme,
            revision: revision.to_string(),
        })
    }

    pub fn update_settings(
        &mut self,
        request: UpdateSettings,
    ) -> Result<SettingsSnapshot, AppError> {
        let revision: i64 = request
            .expected_revision
            .parse()
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        if revision < 0 || revision == i64::MAX || revision.to_string() != request.expected_revision
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let theme = match request.theme {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        };
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE settings SET theme = ?1, revision = revision + 1 WHERE singleton = 1 AND revision = ?2",
            params![theme, revision],
        )?;
        if changed != 1 {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        tx.commit()?;
        self.settings()
    }

    pub fn checkpoint(&self) -> Result<(), AppError> {
        // Commits are already durable in WAL; checkpoint is maintenance, not the commit boundary.
        self.connection
            .execute_batch("PRAGMA wal_checkpoint(PASSIVE)")?;
        Ok(())
    }
}
