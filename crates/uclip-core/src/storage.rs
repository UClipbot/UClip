use std::{path::Path, time::Duration};

use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};

use crate::{AppError, ErrorCode, Language, SettingsSnapshot, Theme, UpdateSettings};

pub const SCHEMA_VERSION: u32 = 18;
const APPLICATION_ID: i64 = 0x434c4e58; // Stable database format ID (CLNX), independent of the product name.

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
            INSERT INTO capture_policy VALUES(1,1,'',500,0);
            CREATE TABLE history_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            INSERT INTO history_meta VALUES(1,0);
            CREATE TABLE clips(id TEXT PRIMARY KEY,kind TEXT NOT NULL CHECK(kind IN ('text','image','files')),text TEXT NOT NULL,data BLOB NOT NULL,width INTEGER,height INTEGER,file_count INTEGER NOT NULL DEFAULT 0,fingerprint BLOB NOT NULL,origin TEXT NOT NULL DEFAULT 'capture',pinned INTEGER NOT NULL DEFAULT 0,deleted INTEGER NOT NULL DEFAULT 0,collection TEXT,captured_at INTEGER NOT NULL,last_used_at INTEGER,sequence INTEGER NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            CREATE INDEX clips_order ON clips(deleted,sequence DESC,id);
            CREATE INDEX clips_fingerprint ON clips(fingerprint);")?;
            tx.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            // Coalesced outbox: bounded by current identities plus unacknowledged tombstones.
            // It has no clipboard body and is not a synchronization protocol.
            tx.execute_batch("CREATE TABLE clip_changes(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,operation TEXT NOT NULL CHECK(operation IN ('upsert','delete','purge')),ordinal INTEGER NOT NULL) STRICT;
            INSERT INTO clip_changes SELECT id,revision,CASE WHEN deleted=0 THEN 'upsert' ELSE 'delete' END,sequence FROM clips;
            CREATE TRIGGER clips_change_insert AFTER INSERT ON clips BEGIN
                INSERT INTO clip_changes VALUES(NEW.id,NEW.revision,CASE WHEN NEW.deleted=0 THEN 'upsert' ELSE 'delete' END,(SELECT revision+1 FROM history_meta WHERE singleton=1))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,operation=excluded.operation,ordinal=excluded.ordinal;
            END;
            CREATE TRIGGER clips_change_update AFTER UPDATE ON clips BEGIN
                INSERT INTO clip_changes VALUES(NEW.id,NEW.revision,CASE WHEN NEW.deleted=0 THEN 'upsert' ELSE 'delete' END,(SELECT revision+1 FROM history_meta WHERE singleton=1))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,operation=excluded.operation,ordinal=excluded.ordinal;
            END;
            CREATE TRIGGER clips_change_delete AFTER DELETE ON clips BEGIN
                INSERT INTO clip_changes VALUES(OLD.id,OLD.revision+1,'purge',(SELECT revision+1 FROM history_meta WHERE singleton=1))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,operation=excluded.operation,ordinal=excluded.ordinal;
            END;
            CREATE TABLE desktop_preferences(singleton INTEGER PRIMARY KEY CHECK(singleton=1),close_to_tray INTEGER NOT NULL CHECK(close_to_tray IN (0,1)),always_on_top INTEGER NOT NULL CHECK(always_on_top IN (0,1)),auto_hide INTEGER NOT NULL CHECK(auto_hide IN (0,1)),shortcut TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            INSERT INTO desktop_preferences VALUES(1,0,0,0,'CommandOrControl+Shift+V',0);
            CREATE TABLE window_placement(singleton INTEGER PRIMARY KEY CHECK(singleton=1),x INTEGER NOT NULL,y INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,maximized INTEGER NOT NULL CHECK(maximized IN (0,1))) STRICT;")?;
            tx.pragma_update(None, "user_version", 3)?;
        }
        if version < 4 {
            tx.execute_batch("CREATE TABLE keyboard_bindings(singleton INTEGER PRIMARY KEY CHECK(singleton=1),value TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;")?;
            tx.execute(
                "INSERT INTO keyboard_bindings VALUES(1,?1,0)",
                [serde_json::to_string(&crate::KeyBindings::default())
                    .map_err(|_| AppError::new(ErrorCode::Storage))?],
            )?;
            tx.pragma_update(None, "user_version", 4)?;
        }
        if version < 5 {
            tx.execute_batch("CREATE TABLE collections(id TEXT PRIMARY KEY,name TEXT NOT NULL,name_key TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0 CHECK(deleted IN (0,1)),sequence INTEGER NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0)) STRICT;
            CREATE UNIQUE INDEX collection_names ON collections(name_key) WHERE deleted=0;
            CREATE TABLE collection_changes(id TEXT PRIMARY KEY,revision INTEGER NOT NULL,operation TEXT NOT NULL CHECK(operation IN ('upsert','delete','purge')),ordinal INTEGER NOT NULL) STRICT;
            CREATE TRIGGER collections_change_insert AFTER INSERT ON collections BEGIN
                INSERT INTO collection_changes VALUES(NEW.id,NEW.revision,CASE WHEN NEW.deleted=0 THEN 'upsert' ELSE 'delete' END,(SELECT revision+1 FROM history_meta WHERE singleton=1))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,operation=excluded.operation,ordinal=excluded.ordinal;
            END;
            CREATE TRIGGER collections_change_update AFTER UPDATE ON collections BEGIN
                INSERT INTO collection_changes VALUES(NEW.id,NEW.revision,CASE WHEN NEW.deleted=0 THEN 'upsert' ELSE 'delete' END,(SELECT revision+1 FROM history_meta WHERE singleton=1))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,operation=excluded.operation,ordinal=excluded.ordinal;
            END;
            CREATE TRIGGER collections_change_delete AFTER DELETE ON collections BEGIN
                INSERT INTO collection_changes VALUES(OLD.id,OLD.revision+1,'purge',(SELECT revision+1 FROM history_meta WHERE singleton=1))
                ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,operation=excluded.operation,ordinal=excluded.ordinal;
            END;
            CREATE INDEX clips_collection_order ON clips(collection,deleted,pinned DESC,sequence DESC,id);")?;
            tx.pragma_update(None, "user_version", 5)?;
        }
        if version < 6 {
            tx.execute_batch("ALTER TABLE desktop_preferences ADD COLUMN native_titlebar INTEGER NOT NULL DEFAULT 1 CHECK(native_titlebar IN (0,1));")?;
            tx.pragma_update(None, "user_version", 6)?;
        }
        if version < 7 {
            // Existing installations keep their OS autostart choice, including disabled.
            tx.execute_batch("ALTER TABLE desktop_preferences ADD COLUMN autostart_initialized INTEGER NOT NULL DEFAULT 1 CHECK(autostart_initialized IN (0,1));")?;
            if version == 0 {
                tx.execute_batch("UPDATE desktop_preferences SET close_to_tray=1,always_on_top=1,native_titlebar=0,autostart_initialized=0 WHERE singleton=1;")?;
            }
            tx.pragma_update(None, "user_version", 7)?;
        }
        if version < 8 {
            tx.execute_batch("ALTER TABLE clips ADD COLUMN source_app TEXT CHECK(source_app IS NULL OR length(CAST(source_app AS BLOB)) BETWEEN 1 AND 256);")?;
            tx.pragma_update(None, "user_version", 8)?;
        }
        if version < 9 {
            tx.execute_batch("ALTER TABLE settings ADD COLUMN language TEXT NOT NULL DEFAULT 'system' CHECK(language IN ('system','zh-CN','en'));")?;
            tx.pragma_update(None, "user_version", 9)?;
        }
        if version < 10 {
            // Pinning controls blur hiding only; native windows are always topmost.
            // Retain the legacy column; only auto_hide remains a preference.
            tx.execute_batch("UPDATE desktop_preferences SET auto_hide=CASE WHEN always_on_top=1 THEN 0 ELSE auto_hide END,always_on_top=1;")?;
            tx.pragma_update(None, "user_version", 10)?;
        }
        if version < 11 {
            // Enable the former untouched default once; explicit user choices
            // always advance revision and must survive upgrades and restarts.
            tx.execute_batch("UPDATE capture_policy SET enabled=1,revision=revision+1 WHERE singleton=1 AND enabled=0 AND revision=0;")?;
            tx.pragma_update(None, "user_version", 11)?;
        }
        if version < 12 {
            if version >= 4 {
                crate::KeyBindings::migrate_search_default(&tx)?;
            }
            tx.pragma_update(None, "user_version", 12)?;
        }
        if version < 13 {
            tx.execute_batch("ALTER TABLE settings ADD COLUMN show_search_by_default INTEGER NOT NULL DEFAULT 1 CHECK(show_search_by_default IN (0,1));")?;
            tx.pragma_update(None, "user_version", 13)?;
        }
        if version < 14 {
            tx.execute_batch(
                "CREATE TABLE quick_paste_preferences (
                singleton INTEGER PRIMARY KEY CHECK(singleton=1),
                shortcut TEXT CHECK(shortcut IS NULL OR length(shortcut) BETWEEN 1 AND 100),
                include_pinned INTEGER NOT NULL CHECK(include_pinned IN (0,1)),
                revision INTEGER NOT NULL CHECK(revision>=0)
            ) STRICT; INSERT INTO quick_paste_preferences VALUES(1,NULL,0,0);",
            )?;
            tx.pragma_update(None, "user_version", 14)?;
        }
        if version < 15 {
            tx.execute_batch(
                "CREATE TABLE auxiliary_window_sizes (
                    window TEXT PRIMARY KEY CHECK(window IN ('settings', 'quick-paste')),
                    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 4096),
                    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 4096)
                ) STRICT;",
            )?;
            tx.pragma_update(None, "user_version", 15)?;
        }
        if version < 16 {
            crate::replica::migrate(&tx)?;
            tx.pragma_update(None, "user_version", 16)?;
        }
        if version < 17 {
            if version == 16 {
                crate::replica::validate_triggers(&tx)?;
                crate::replica::migrate_legacy_grants(&tx)?;
            }
            crate::replica::migrate_frontier(&tx)?;
            tx.pragma_update(None, "user_version", 17)?;
        }
        if version < 18 {
            if version == 17 {
                crate::replica::validate_triggers(&tx)?;
            }
            crate::replica::migrate_latest(&tx)?;
            tx.pragma_update(None, "user_version", 18)?;
        }
        tx.commit()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let storage = Self { connection };
        storage.capture_policy()?;
        storage.quick_paste_preferences()?;
        storage.auxiliary_window_size(crate::AuxiliaryWindow::Settings)?;
        storage
            .connection
            .prepare("SELECT source_app FROM clips LIMIT 0")?;
        storage.history_revision()?;
        storage.desktop_preferences()?;
        storage.connection.query_row(
            "SELECT autostart_initialized FROM desktop_preferences WHERE singleton=1",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        storage.keyboard_bindings()?;
        storage.collections()?;
        storage
            .connection
            .prepare("SELECT id,revision,operation,ordinal FROM collection_changes LIMIT 0")?;
        let collection_triggers: u32 = storage.connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN ('collections_change_insert','collections_change_update','collections_change_delete')", [], |r|r.get(0))?;
        if collection_triggers != 3 {
            return Err(AppError::new(ErrorCode::Storage));
        }
        storage
            .connection
            .prepare("SELECT id,revision,operation,ordinal FROM clip_changes LIMIT 0")?;
        let triggers: u32 = storage.connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name IN ('clips_change_insert','clips_change_update','clips_change_delete')", [], |r|r.get(0))?;
        if triggers != 3 {
            return Err(AppError::new(ErrorCode::Storage));
        }
        storage.settings()?; // Fail startup on an invalid persisted setting.
        storage.validate_replica_schema()?;
        Ok(storage)
    }

    pub fn settings(&self) -> Result<SettingsSnapshot, AppError> {
        let (theme, language, revision, show_search): (String, String, i64, i64) =
            self.connection.query_row(
                "SELECT theme, language, revision, show_search_by_default FROM settings WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        let theme = match theme.as_str() {
            "system" => Theme::System,
            "light" => Theme::Light,
            "dark" => Theme::Dark,
            _ => return Err(AppError::new(ErrorCode::Storage)),
        };
        let language = match language.as_str() {
            "system" => Language::System,
            "zh-CN" => Language::ZhCn,
            "en" => Language::En,
            _ => return Err(AppError::new(ErrorCode::Storage)),
        };
        if revision < 0 || !matches!(show_search, 0 | 1) {
            return Err(AppError::new(ErrorCode::Storage));
        }
        Ok(SettingsSnapshot {
            theme,
            language,
            show_search_by_default: show_search == 1,
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
        let language = request.language.map(|value| match value {
            Language::System => "system",
            Language::ZhCn => "zh-CN",
            Language::En => "en",
        });
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE settings SET theme = ?1, language = coalesce(?3, language), show_search_by_default = coalesce(?4, show_search_by_default), revision = revision + 1 WHERE singleton = 1 AND revision = ?2",
            params![theme, revision, language, request.show_search_by_default],
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
