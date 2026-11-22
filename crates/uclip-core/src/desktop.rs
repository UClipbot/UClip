use crate::{AppError, DesktopPreferences, ErrorCode, Storage, UpdateDesktopPreferences};
use rusqlite::{params, OptionalExtension};

/// Native window coordinates only; no frontend command accepts placement or file paths.
#[derive(Debug, Clone, Copy)]
pub struct WindowPlacement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuxiliaryWindow {
    Settings,
    QuickPaste,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuxiliaryWindowSize {
    pub width: u32,
    pub height: u32,
}

impl AuxiliaryWindow {
    pub const MAXIMUM_DIMENSION: u32 = 4096;

    pub fn label(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::QuickPaste => "quick-paste",
        }
    }

    pub fn default_size(self) -> AuxiliaryWindowSize {
        match self {
            Self::Settings => AuxiliaryWindowSize {
                width: 720,
                height: 640,
            },
            Self::QuickPaste => AuxiliaryWindowSize {
                width: 520,
                height: 360,
            },
        }
    }

    pub fn minimum_size(self) -> AuxiliaryWindowSize {
        match self {
            Self::Settings => AuxiliaryWindowSize {
                width: 560,
                height: 460,
            },
            Self::QuickPaste => AuxiliaryWindowSize {
                width: 360,
                height: 290,
            },
        }
    }

    pub fn accepts(self, size: AuxiliaryWindowSize) -> bool {
        let minimum = self.minimum_size();
        (minimum.width..=Self::MAXIMUM_DIMENSION).contains(&size.width)
            && (minimum.height..=Self::MAXIMUM_DIMENSION).contains(&size.height)
    }
}

impl Storage {
    /// Consume the fresh-install default before an OS side effect. Never re-enable a
    /// user's disabled login item on later launches, including after a failed attempt.
    pub fn take_autostart_default(&mut self) -> Result<bool, AppError> {
        Ok(self.connection.execute(
            "UPDATE desktop_preferences SET autostart_initialized=1 WHERE singleton=1 AND autostart_initialized=0",
            [],
        )? == 1)
    }

    pub fn desktop_preferences(&self) -> Result<DesktopPreferences, AppError> {
        Ok(self.connection.query_row("SELECT close_to_tray,auto_hide,shortcut,revision,native_titlebar FROM desktop_preferences WHERE singleton=1", [], |r| Ok(DesktopPreferences {
            close_to_tray: r.get(0)?, auto_hide: r.get(1)?, shortcut: r.get(2)?, revision: r.get::<_, i64>(3)?.to_string(), native_titlebar: r.get(4)?
        }))?)
    }

    pub fn update_desktop_preferences(
        &mut self,
        request: UpdateDesktopPreferences,
    ) -> Result<DesktopPreferences, AppError> {
        let revision: i64 = request
            .expected_revision
            .parse()
            .map_err(|_| AppError::new(ErrorCode::InvalidInput))?;
        if revision < 0
            || revision == i64::MAX
            || revision.to_string() != request.expected_revision
            || request.shortcut.is_empty()
            || request.shortcut.len() > 100
            || request.shortcut.chars().any(char::is_control)
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        let tx = self.connection.transaction()?;
        if tx.execute("UPDATE desktop_preferences SET close_to_tray=?1,auto_hide=?2,shortcut=?3,native_titlebar=?4,revision=revision+1 WHERE singleton=1 AND revision=?5", params![request.close_to_tray,request.auto_hide,request.shortcut,request.native_titlebar,revision])? != 1 {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        tx.commit()?;
        self.desktop_preferences()
    }

    pub fn quick_paste_preferences(&self) -> Result<crate::QuickPastePreferences, AppError> {
        Ok(self.connection.query_row(
            "SELECT shortcut,include_pinned,revision FROM quick_paste_preferences WHERE singleton=1",
            [],
            |r| Ok(crate::QuickPastePreferences {
                shortcut: r.get(0)?,
                include_pinned: r.get(1)?,
                revision: r.get::<_, i64>(2)?.to_string(),
            }),
        )?)
    }

    pub fn update_quick_paste_preferences(
        &mut self,
        request: crate::UpdateQuickPastePreferences,
    ) -> Result<crate::QuickPastePreferences, AppError> {
        let revision = crate::history::mutation_revision(&request.expected_revision)?;
        if request.shortcut.as_ref().is_some_and(|value| {
            value.trim().is_empty() || value.len() > 100 || value.chars().any(char::is_control)
        }) {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        if self.connection.execute(
            "UPDATE quick_paste_preferences SET shortcut=?1,include_pinned=?2,revision=revision+1
             WHERE singleton=1 AND revision=?3",
            params![request.shortcut, request.include_pinned, revision],
        )? != 1
        {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        self.quick_paste_preferences()
    }

    pub fn auxiliary_window_size(
        &self,
        window: AuxiliaryWindow,
    ) -> Result<AuxiliaryWindowSize, AppError> {
        let saved = self
            .connection
            .query_row(
                "SELECT width,height FROM auxiliary_window_sizes WHERE window=?1",
                [window.label()],
                |row| {
                    Ok(AuxiliaryWindowSize {
                        width: row.get(0)?,
                        height: row.get(1)?,
                    })
                },
            )
            .optional()?;
        let size = saved.unwrap_or_else(|| window.default_size());
        if !window.accepts(size) {
            return Err(AppError::new(ErrorCode::Storage));
        }
        Ok(size)
    }

    pub fn save_auxiliary_window_size(
        &mut self,
        window: AuxiliaryWindow,
        size: AuxiliaryWindowSize,
    ) -> Result<(), AppError> {
        if !window.accepts(size) {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        self.connection.execute(
            "INSERT INTO auxiliary_window_sizes (window,width,height) VALUES(?1,?2,?3)
             ON CONFLICT(window) DO UPDATE SET width=excluded.width,height=excluded.height",
            params![window.label(), size.width, size.height],
        )?;
        Ok(())
    }

    pub fn window_placement(&self) -> Result<Option<WindowPlacement>, AppError> {
        Ok(self
            .connection
            .query_row(
                "SELECT x,y,width,height,maximized FROM window_placement WHERE singleton=1",
                [],
                |r| {
                    Ok(WindowPlacement {
                        x: r.get(0)?,
                        y: r.get(1)?,
                        width: r.get(2)?,
                        height: r.get(3)?,
                        maximized: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    pub fn save_window_placement(&mut self, placement: WindowPlacement) -> Result<(), AppError> {
        if placement.width == 0
            || placement.height == 0
            || placement.width > 32768
            || placement.height > 32768
        {
            return Err(AppError::new(ErrorCode::InvalidInput));
        }
        self.connection.execute("INSERT INTO window_placement VALUES(1,?1,?2,?3,?4,?5) ON CONFLICT(singleton) DO UPDATE SET x=excluded.x,y=excluded.y,width=excluded.width,height=excluded.height,maximized=excluded.maximized", params![placement.x,placement.y,placement.width,placement.height,placement.maximized])?;
        Ok(())
    }
}
