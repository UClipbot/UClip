use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const CONTRACT_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Presentation preference; unsupported system languages fall back to English.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum Language {
    #[serde(rename = "system")]
    System,
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsSnapshot {
    pub theme: Theme,
    pub language: Language,
    pub show_search_by_default: bool,
    /// Decimal string: revisions must remain exact in JavaScript.
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateSettings {
    pub theme: Theme,
    /// Omitted by older clients when changing only the theme.
    pub language: Option<Language>,
    pub show_search_by_default: Option<bool>,
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
    LimitExceeded,
    InvalidPattern,
    NotFound,
    Clipboard,
    MissingFile,
    Restricted,
    TargetLost,
    ShortcutConflict,
    QueryTimeout,
    Excluded,
    DuplicateName,
    SyncConnection,
    SyncAuthentication,
    SyncData,
    FilePeerOffline,
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
            ErrorCode::Conflict => "数据已发生变化，请刷新后重试。",
            ErrorCode::InvalidInput => "请求参数无效。",
            ErrorCode::Busy => "本地服务繁忙，请稍后重试。",
            ErrorCode::LimitExceeded => "内容或历史容量超出限制，未截断保存。",
            ErrorCode::InvalidPattern => "搜索或排除表达式无效；不支持回溯引用和环视。",
            ErrorCode::NotFound => "记录已不存在，请刷新。",
            ErrorCode::Clipboard => "系统剪贴板暂不可用，请重试。",
            ErrorCode::MissingFile => "部分文件或文件夹已失效，未写入剪贴板。",
            ErrorCode::Restricted => "当前平台或权限不支持此操作。",
            ErrorCode::TargetLost => "原应用目标已变化，未自动粘贴。",
            ErrorCode::ShortcutConflict => "快捷键重复、被占用或属于系统保留组合，原设置保持不变。",
            ErrorCode::Excluded => "内容命中排除规则，未保存。",
            ErrorCode::DuplicateName => "分类名称已存在，请使用其他名称。",
            ErrorCode::QueryTimeout => "搜索超时，请缩小范围或简化表达式。",
            ErrorCode::SyncConnection => "无法连接同步设备或存储服务。",
            ErrorCode::SyncAuthentication => "同步认证失败，请检查配对码或服务凭据。",
            ErrorCode::SyncData => "同步数据不完整或无法读取。",
            ErrorCode::FilePeerOffline => "文件来源设备不在线。",
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
            rusqlite::Error::SqliteFailure(ref inner, Some(ref message))
                if inner.code == rusqlite::ErrorCode::ConstraintViolation
                    && message == "uclip_replica_clock_limit" =>
            {
                ErrorCode::LimitExceeded
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClipKind {
    Text,
    Image,
    Files,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapturePolicy {
    pub enabled: bool,
    pub exclude_pattern: String,
    pub max_items: u32,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateCapturePolicy {
    pub enabled: bool,
    pub exclude_pattern: String,
    pub max_items: u32,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryQuery {
    pub query: String,
    pub regex: bool,
    pub kind: Option<ClipKind>,
    #[serde(default)]
    pub pinned_only: bool,
    pub collection_id: Option<String>,
    pub offset: u32,
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItem {
    pub manual: bool,
    pub source_app: Option<String>,
    pub pinned: bool,
    pub collection_id: Option<String>,
    pub id: String,
    pub kind: ClipKind,
    pub preview: String,
    pub captured_at: String,
    pub last_used_at: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub file_count: u32,
    pub revision: String,
    /// UTF-16 offsets into preview. Matching stays in the bounded Rust regex engine.
    pub match_ranges: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub total: u32,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipDetail {
    pub id: String,
    pub text: String,
    pub image_data_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClipVersion {
    pub id: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EditableContent {
    Text(String),
    Files(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditClip {
    pub clip: ClipVersion,
    pub content: EditableContent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardStatus {
    pub policy: CapturePolicy,
    pub history_revision: String,
    pub available: bool,
    pub error: Option<AppError>,
    pub paste_available: bool,
    pub paste_target: bool,
    pub shortcut_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CopyResult {
    pub copied: bool,
    pub usage_saved: bool,
    pub paste_requested: bool,
    pub warning: Option<AppError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeClipDeletion {
    pub id: String,
    pub expected_revision: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipDeletionSnapshot {
    pub id: String,
    pub revision: String,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum PurgeTrash {
    Selected(PurgeSelectedTrash),
    All(EmptyTrash),
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PurgeSelectedTrash {
    pub expected_revision: String,
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmptyTrash {
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChangeClipPin {
    pub id: String,
    pub expected_revision: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipPinSnapshot {
    pub id: String,
    pub revision: String,
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeduplicatePinned {
    /// Revision of the whole history, including pinned items outside the visible page.
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DeduplicationSnapshot {
    pub removed: u32,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CollectionItem {
    pub id: String,
    pub name: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CollectionsSnapshot {
    pub items: Vec<CollectionItem>,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateCollection {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RenameCollection {
    pub id: String,
    pub name: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeleteCollection {
    pub id: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TransferMode {
    Move,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferClip {
    pub id: String,
    pub expected_revision: String,
    pub collection_id: Option<String>,
    pub mode: TransferMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransferClips {
    pub clips: Vec<ClipVersion>,
    pub collection_id: Option<String>,
    pub mode: TransferMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipTransferBatch {
    pub completed: Vec<ClipTransferSnapshot>,
    pub failure: Option<AppError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ClipTransferSnapshot {
    pub id: String,
    pub revision: String,
    pub collection_id: Option<String>,
    pub mode: TransferMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopPreferences {
    pub close_to_tray: bool,
    pub auto_hide: bool,
    pub native_titlebar: bool,
    pub shortcut: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateDesktopPreferences {
    pub close_to_tray: bool,
    pub auto_hide: bool,
    pub native_titlebar: bool,
    pub shortcut: String,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DesktopStatus {
    pub keyboard: crate::KeyboardSnapshot,
    pub preferences: DesktopPreferences,
    pub tray_available: bool,
    pub shortcut_available: bool,
    pub autostart_enabled: Option<bool>,
    pub hardware_acceleration: Option<HardwareAcceleration>,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HardwareAcceleration {
    pub enabled: bool,
    pub enabled_at_startup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuickPastePreferences {
    pub shortcut: Option<String>,
    pub include_pinned: bool,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateQuickPastePreferences {
    pub shortcut: Option<String>,
    pub include_pinned: bool,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuickPasteSettings {
    pub preferences: QuickPastePreferences,
    pub available: bool,
    pub error: Option<AppError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuickPasteItem {
    pub kind: ClipKind,
    pub text: String,
    pub file_count: Option<u32>,
    pub thumbnail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuickPasteSnapshot {
    pub token: String,
    pub items: Vec<QuickPasteItem>,
    pub loading: bool,
    pub copy_only: bool,
    pub feedback_only: bool,
    pub final_key: Option<String>,
    pub main_shortcut: String,
    pub main_code: String,
    pub quick_code: String,
    pub main_modifiers: u8,
    pub quick_modifiers: u8,
    pub error: Option<AppError>,
    pub result: Option<CopyResult>,
}

/// Logical coordinates relative to the local history WebView.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PreviewAnchor {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Presentation metadata only. The preview reads content through the core detail use case.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct HoverPreviewRequest {
    pub token: String,
    pub id: String,
    pub kind: ClipKind,
    pub source: String,
    pub saved_at: String,
    pub size: Option<String>,
    pub anchor: PreviewAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SyncRole {
    Off,
    Discovering,
    Host,
    Client,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SyncRemoteInput {
    Webdav {
        url: String,
        username: String,
        password: String,
    },
    Sftp {
        host: String,
        port: u16,
        directory: String,
        username: String,
        password: String,
        host_key: String,
    },
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SyncRemoteSettings {
    Webdav {
        url: String,
        username: String,
    },
    Sftp {
        host: String,
        port: u16,
        directory: String,
        username: String,
        host_key: String,
    },
}

#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub paired: bool,
    pub lan_enabled: bool,
    pub role: SyncRole,
    pub peers: u32,
    pub lan_last_sync: Option<String>,
    pub lan_error: Option<AppError>,
    pub remote: Option<SyncRemoteSettings>,
    pub remote_syncing: bool,
    pub remote_last_sync: Option<String>,
    pub remote_error: Option<AppError>,
}

#[derive(Deserialize, TS)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum UpdateSync {
    Create,
    CreateFromNow,
    Join { code: String },
    JoinFromNow { code: String },
    Lan { enabled: bool },
    Remote { remote: Option<SyncRemoteInput> },
    SyncRemoteNow,
}
