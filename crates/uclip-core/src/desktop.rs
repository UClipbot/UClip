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

impl Storage {
    pub fn desktop_preferences(&self) -> Result<DesktopPreferences, AppError> {
        Ok(self.connection.query_row("SELECT close_to_tray,always_on_top,auto_hide,shortcut,revision,native_titlebar FROM desktop_preferences WHERE singleton=1", [], |r| Ok(DesktopPreferences {
            close_to_tray: r.get(0)?, always_on_top: r.get(1)?, auto_hide: r.get(2)?, shortcut: r.get(3)?, revision: r.get::<_, i64>(4)?.to_string(), native_titlebar: r.get(5)?
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
        if tx.execute("UPDATE desktop_preferences SET close_to_tray=?1,always_on_top=?2,auto_hide=?3,shortcut=?4,native_titlebar=?5,revision=revision+1 WHERE singleton=1 AND revision=?6", params![request.close_to_tray,request.always_on_top,request.auto_hide,request.shortcut,request.native_titlebar,revision])? != 1 {
            return Err(AppError::new(ErrorCode::Conflict));
        }
        tx.commit()?;
        self.desktop_preferences()
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
