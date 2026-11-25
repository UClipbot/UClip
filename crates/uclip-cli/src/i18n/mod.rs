pub mod messages;

use std::{env, io, sync::OnceLock};

use clap::error::{ContextKind, ErrorKind};
use uclip_core::{AppError, ErrorCode, SyncRole};

use messages::Message;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Locale {
    English,
    SimplifiedChinese,
}

impl Locale {
    fn from_tag(language: &str) -> Self {
        let primary = language.split(['-', '_', '.', '@']).next();
        if primary.is_some_and(|value| value.eq_ignore_ascii_case("zh")) {
            Self::SimplifiedChinese
        } else {
            Self::English
        }
    }
}

fn current() -> Locale {
    static LOCALE: OnceLock<Locale> = OnceLock::new();
    *LOCALE.get_or_init(|| match env::var("UCLIP_LANG").as_deref() {
        Ok("en") => Locale::English,
        Ok("zh-CN") => Locale::SimplifiedChinese,
        _ => ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|key| env::var(key).ok())
            .find(|value| !value.is_empty())
            .map_or_else(system_locale, |value| Locale::from_tag(&value)),
    })
}

#[cfg(target_os = "macos")]
fn system_locale() -> Locale {
    use objc2_foundation::NSLocale;

    NSLocale::preferredLanguages()
        .firstObject()
        .map_or(Locale::English, |language| {
            Locale::from_tag(&language.to_string())
        })
}

#[cfg(target_os = "windows")]
fn system_locale() -> Locale {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;

    // The low ten bits of a Windows language ID identify the primary language.
    // GetUserDefaultUILanguage takes no pointers and has no caller preconditions.
    if unsafe { GetUserDefaultUILanguage() } & 0x03ff == 0x04 {
        Locale::SimplifiedChinese
    } else {
        Locale::English
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn system_locale() -> Locale {
    Locale::English
}

pub fn text((english, chinese): Message) -> &'static str {
    match current() {
        Locale::English => english,
        Locale::SimplifiedChinese => chinese,
    }
}

pub fn notice(message: Message) {
    eprintln!("uclip: {}", text(message));
}

pub(crate) fn error_name(code: ErrorCode) -> String {
    match serde_json::to_value(code) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unavailable".into(),
    }
}

pub fn app_error(error: &AppError) {
    let name = error_name(error.code);
    match current() {
        Locale::English => eprintln!("uclip: {name}"),
        Locale::SimplifiedChinese => {
            let message = match error.code {
                ErrorCode::NotFound => messages::DATA_NOT_FOUND,
                ErrorCode::Unavailable => messages::SERVICE_UNAVAILABLE,
                _ => &error.message,
            };
            eprintln!("uclip: {name}: {message}");
        }
    }
}

pub fn sync_status(role: SyncRole, peers: u32, lan: Option<ErrorCode>, remote: Option<ErrorCode>) {
    if current() == Locale::English {
        eprintln!("uclip: LAN {role:?}, peers {peers}, LAN error {lan:?}, remote error {remote:?}");
        return;
    }

    use messages::{
        SYNC_CLIENT, SYNC_DISCOVERING, SYNC_HOST, SYNC_LAN_ERROR_LABEL, SYNC_NO_ERROR, SYNC_OFF,
        SYNC_PEERS_LABEL, SYNC_REMOTE_ERROR_LABEL, SYNC_ROLE_LABEL,
    };

    let role = match role {
        SyncRole::Off => SYNC_OFF,
        SyncRole::Discovering => SYNC_DISCOVERING,
        SyncRole::Host => SYNC_HOST,
        SyncRole::Client => SYNC_CLIENT,
    };
    let lan = lan.map_or_else(|| SYNC_NO_ERROR.into(), error_name);
    let remote = remote.map_or_else(|| SYNC_NO_ERROR.into(), error_name);
    eprintln!("uclip: {SYNC_ROLE_LABEL}: {role}, {SYNC_PEERS_LABEL}: {peers}, {SYNC_LAN_ERROR_LABEL}: {lan}, {SYNC_REMOTE_ERROR_LABEL}: {remote}");
}

fn localize_help(help: &str) -> String {
    // Clap generates these labels after the command's localized descriptions are set.
    messages::CLAP_HELP_TEXT
        .iter()
        .fold(help.to_owned(), |help, (english, chinese)| {
            help.replace(english, chinese)
        })
}

pub fn print_parse_error(error: &clap::Error) -> io::Result<()> {
    use messages::*;

    if current() == Locale::English || error.kind() == ErrorKind::DisplayVersion {
        return error.print();
    }

    let kind = error.kind();
    if matches!(
        kind,
        ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    ) {
        use io::Write;
        let help = localize_help(&error.render().to_string());
        return if error.use_stderr() {
            io::stderr().lock().write_all(help.as_bytes())
        } else {
            io::stdout().lock().write_all(help.as_bytes())
        };
    }

    let message = match kind {
        ErrorKind::UnknownArgument => ARG_UNKNOWN,
        ErrorKind::InvalidSubcommand => SUBCOMMAND_UNKNOWN,
        ErrorKind::ArgumentConflict => ARG_CONFLICT,
        ErrorKind::MissingRequiredArgument => ARG_REQUIRED,
        ErrorKind::MissingSubcommand => SUBCOMMAND_REQUIRED,
        ErrorKind::TooFewValues | ErrorKind::TooManyValues | ErrorKind::WrongNumberOfValues => {
            ARG_COUNT
        }
        ErrorKind::NoEquals => ARG_EQUALS,
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => ARG_VALUE,
        _ => ARG_INVALID,
    };
    // Show declared options and accepted values without echoing rejected user input.
    use io::Write;
    let mut output = io::stderr().lock();
    write!(output, "{ERROR_LABEL}: {message}")?;
    if let Some(argument) = match kind {
        ErrorKind::UnknownArgument
        | ErrorKind::InvalidSubcommand
        | ErrorKind::MissingSubcommand => None,
        _ => error.get(ContextKind::InvalidArg),
    } {
        write!(output, ": {argument}")?;
    }
    writeln!(output)?;
    for context in [ContextKind::PriorArg, ContextKind::ValidValue] {
        if let Some(detail) = error.get(context) {
            writeln!(output, "  {detail}")?;
        }
    }
    if let Some(usage) = error.get(ContextKind::Usage) {
        writeln!(output, "\n{}", localize_help(&usage.to_string()))?;
    }
    writeln!(output, "\n{ARG_ERROR_HINT}")
}
