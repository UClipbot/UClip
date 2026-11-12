use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const CONTRACT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsSnapshot {
    pub theme: Theme,
    /// Decimal string: revisions must remain exact in JavaScript.
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSettings {
    pub theme: Theme,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Storage,
    UnsupportedSchema,
    ForeignDatabase,
    Conflict,
    InvalidInput,
    Busy,
    Unavailable,
}

/// Safe across IPC. Never expose SQLite errors, paths, or user content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode) -> Self {
        let message = match code {
            ErrorCode::Storage => "本地存储操作失败，请检查磁盘空间及目录权限。",
            ErrorCode::UnsupportedSchema => "数据由较新版本创建，请使用较新版本打开。",
            ErrorCode::ForeignDatabase => "目标文件不属于此应用，已停止打开。",
            ErrorCode::Conflict => "设置已发生变化，请刷新后重试。",
            ErrorCode::InvalidInput => "请求参数无效。",
            ErrorCode::Busy => "本地服务繁忙，请稍后重试。",
            ErrorCode::Unavailable => "本地服务不可用，请重启应用。",
        };
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        let code = match error {
            rusqlite::Error::SqliteFailure(ref inner, _)
                if matches!(
                    inner.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                ErrorCode::Busy
            }
            _ => ErrorCode::Storage,
        };
        Self::new(code)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityState {
    Available,
    Unverified,
    Restricted,
    PermissionRequired,
    NotImplemented,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapability {
    pub id: String,
    pub state: CapabilityState,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PlatformReport {
    pub os: String,
    pub session_hint: String,
    pub capabilities: Vec<PlatformCapability>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSnapshot {
    pub contract_version: u32,
    pub app_version: String,
    pub data_profile: String,
    pub schema_version: u32,
    pub settings: SettingsSnapshot,
    pub platform: PlatformReport,
}
