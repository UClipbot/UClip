use crate::{AppError, ErrorCode, Storage};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyChord {
    pub code: String,
    pub primary: bool,
    pub alt: bool,
    pub shift: bool,
}
impl KeyChord {
    fn plain(code: &str) -> Self {
        Self {
            code: code.into(),
            primary: false,
            alt: false,
            shift: false,
        }
    }
    fn primary(code: &str) -> Self {
        Self {
            primary: true,
            ..Self::plain(code)
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyBindings {
    pub search: KeyChord,
    pub settings: KeyChord,
    pub copy: KeyChord,
    pub preview: KeyChord,
    pub remove: KeyChord,
    pub up: KeyChord,
    pub down: KeyChord,
    pub dismiss: KeyChord,
}
impl Default for KeyBindings {
    fn default() -> Self {
        Self {
            search: KeyChord::primary("KeyF"),
            settings: KeyChord::primary("Comma"),
            copy: KeyChord::plain("Enter"),
            preview: KeyChord::plain("Space"),
            remove: KeyChord::plain("Delete"),
            up: KeyChord::plain("ArrowUp"),
            down: KeyChord::plain("ArrowDown"),
            dismiss: KeyChord::plain("Escape"),
        }
    }
}
impl KeyBindings {
    pub(crate) fn migrate_search_default(tx: &rusqlite::Transaction<'_>) -> Result<(), AppError> {
        let (value, revision): (String, i64) = tx.query_row(
            "SELECT value,revision FROM keyboard_bindings WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let bindings: Self =
            serde_json::from_str(&value).map_err(|_| AppError::new(ErrorCode::Storage))?;
        bindings
            .validate()
            .map_err(|_| AppError::new(ErrorCode::Storage))?;
        let defaults = Self::default();
        let legacy = Self {
            search: KeyChord::primary("KeyK"),
            ..defaults.clone()
        };
        // A saved map may deliberately retain Ctrl/Cmd+K or assign Ctrl/Cmd+F elsewhere.
        if revision == 0 && bindings == legacy {
            let value =
                serde_json::to_string(&defaults).map_err(|_| AppError::new(ErrorCode::Storage))?;
            tx.execute(
                "UPDATE keyboard_bindings SET value=?1,revision=revision+1 WHERE singleton=1",
                [value],
            )?;
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), AppError> {
        let mut seen = std::collections::HashSet::new();
        for (action, binding) in [
            ("search", &self.search),
            ("settings", &self.settings),
            ("copy", &self.copy),
            ("preview", &self.preview),
            ("remove", &self.remove),
            ("up", &self.up),
            ("down", &self.down),
            ("dismiss", &self.dismiss),
        ] {
            let code = binding.code.as_str();
            let letter = code.len() == 4
                && code.starts_with("Key")
                && code.as_bytes()[3].is_ascii_uppercase();
            let digit =
                code.len() == 6 && code.starts_with("Digit") && code.as_bytes()[5].is_ascii_digit();
            let function = code
                .strip_prefix('F')
                .and_then(|n| n.parse::<u8>().ok())
                .is_some_and(|n| (1..=12).contains(&n) && code == format!("F{n}"));
            let special = matches!(
                code,
                "Enter" | "Space" | "Escape" | "Delete" | "Backspace" | "ArrowUp" | "ArrowDown"
            );
            let punctuation = matches!(code, "Comma" | "Period" | "Slash");
            let valid = (letter || digit || function || special || punctuation)
                && (binding.primary || binding.alt || special || function);
            let global_allowed = !matches!(action, "search" | "settings" | "dismiss")
                || binding.primary
                || function
                || (action == "dismiss" && code == "Escape");
            let reserved = !global_allowed
                || (code == "Escape" && action != "dismiss")
                || (binding.primary
                    && matches!(
                        code,
                        "KeyA"
                            | "KeyC"
                            | "KeyV"
                            | "KeyX"
                            | "KeyZ"
                            | "KeyY"
                            | "Backspace"
                            | "Delete"
                    ))
                || (binding.alt && code == "F4")
                || (binding.primary && matches!(code, "KeyQ" | "KeyW" | "KeyH"));
            if !valid || reserved || !seen.insert(binding) {
                return Err(AppError::new(ErrorCode::ShortcutConflict));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyboardSnapshot {
    pub bindings: KeyBindings,
    pub revision: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateKeyBindings {
    pub bindings: KeyBindings,
    pub expected_revision: String,
}
impl Storage {
    pub fn keyboard_bindings(&self) -> Result<KeyboardSnapshot, AppError> {
        let (value, revision): (String, i64) = self.connection.query_row(
            "SELECT value,revision FROM keyboard_bindings WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let bindings: KeyBindings =
            serde_json::from_str(&value).map_err(|_| AppError::new(ErrorCode::Storage))?;
        bindings
            .validate()
            .map_err(|_| AppError::new(ErrorCode::Storage))?;
        if revision < 0 {
            return Err(AppError::new(ErrorCode::Storage));
        }
        Ok(KeyboardSnapshot {
            bindings,
            revision: revision.to_string(),
        })
    }
    pub fn update_key_bindings(
        &mut self,
        request: UpdateKeyBindings,
    ) -> Result<KeyboardSnapshot, AppError> {
        request.bindings.validate()?;
        let revision: i64 = request
            .expected_revision
            .parse()
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        if revision < 0 || revision == i64::MAX || revision.to_string() != request.expected_revision
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let value = serde_json::to_string(&request.bindings)
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        let tx = self.connection.transaction()?;
        if tx.execute("UPDATE keyboard_bindings SET value=?1,revision=revision+1 WHERE singleton=1 AND revision=?2",params![value,revision])?!=1 {return Err(AppError::new(ErrorCode::Conflict));}
        tx.commit()?;
        self.keyboard_bindings()
    }
}
