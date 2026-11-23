use std::{io, path::Path, time::Duration};

use clap::Subcommand;
use uclip_core::{AppError, CoreService, ErrorCode, SyncRemoteInput, SyncStatus, UpdateSync};
use uclip_runtime::sync::SyncService;

#[derive(Subcommand)]
pub enum SyncCommand {
    /// Create a new group with LAN discovery enabled.
    Create,
    /// Join an existing group using a pairing code read from stdin.
    Join,
    /// Print the pairing code to stdout for deliberate sharing.
    Code,
    /// Print the saved configuration without credentials.
    Status,
    /// Enable or disable LAN discovery and synchronization.
    Lan {
        #[arg(action = clap::ArgAction::Set)]
        enabled: bool,
    },
    /// Read a WebDAV or SFTP configuration in JSON from stdin.
    #[command(
        long_about = "Read a synchronization configuration in JSON from stdin.\n\nWebDAV fields: kind (webdav), url (HTTPS), username, password.\nSFTP fields: kind (sftp), host, port, directory (absolute), username, password, hostKey (SHA256 fingerprint)."
    )]
    Remote,
    /// Disable remote storage synchronization.
    ClearRemote,
}

pub async fn configure(
    core: &CoreService,
    directory: &Path,
    command: SyncCommand,
) -> Result<(), AppError> {
    let action = match command {
        SyncCommand::Status => return crate::print_json(&SyncService::configured_status(core)?),
        SyncCommand::Code => {
            return crate::history::output(SyncService::configured_code(core)?.as_bytes(), true)
        }
        SyncCommand::Create => UpdateSync::Create,
        SyncCommand::Join => {
            let bytes = crate::history::read_bounded(io::stdin().lock(), 128)?;
            UpdateSync::Join {
                code: String::from_utf8(bytes).map_err(|_| crate::invalid())?,
            }
        }
        SyncCommand::Lan { enabled } => UpdateSync::Lan { enabled },
        SyncCommand::Remote => {
            let bytes = crate::history::read_bounded(io::stdin().lock(), 16_384)?;
            let remote =
                serde_json::from_slice::<SyncRemoteInput>(&bytes).map_err(|_| crate::invalid())?;
            UpdateSync::Remote {
                remote: Some(remote),
            }
        }
        SyncCommand::ClearRemote => UpdateSync::Remote { remote: None },
    };
    let service = SyncService::configure(core.clone(), directory)?;
    crate::print_json(&service.update(action)?)
}

fn report(
    status: &SyncStatus,
) -> (
    uclip_core::SyncRole,
    u32,
    Option<ErrorCode>,
    Option<ErrorCode>,
) {
    (
        status.role,
        status.peers,
        status.lan_error.as_ref().map(|e| e.code),
        status.remote_error.as_ref().map(|e| e.code),
    )
}

pub async fn serve(core: &CoreService, directory: &Path) -> Result<(), AppError> {
    let service = match SyncService::start(core.clone(), directory) {
        Ok(service) => service,
        Err(error) => {
            let _ = core.shutdown().await;
            return Err(error);
        }
    };
    let result = serve_until_signal(&service).await;
    service.stop();
    let closed = core.shutdown().await;
    result.and(closed)
}

async fn serve_until_signal(service: &SyncService) -> Result<(), AppError> {
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|_| crate::unavailable())?;
    let mut interrupt = std::pin::pin!(tokio::signal::ctrl_c());
    #[cfg(unix)]
    let mut stop = std::pin::pin!(async {
        tokio::select! {
            signal = interrupt.as_mut() => signal,
            _ = terminate.recv() => Ok(()),
        }
    });
    #[cfg(not(unix))]
    let mut stop = interrupt.as_mut();
    let initial = service.status()?;
    if !initial.paired || (!initial.lan_enabled && initial.remote.is_none()) {
        eprintln!("uclip: configure a synchronization group first with 'uclip sync create' or 'uclip sync join'");
        return Err(crate::invalid());
    }
    if initial.remote.is_none() {
        if let Some(error) = initial.lan_error {
            return Err(error);
        }
    }
    eprintln!("uclip: synchronization service started");
    let mut previous = None;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            signal = stop.as_mut() => return signal.map_err(|_| crate::unavailable()),
            _ = interval.tick() => {
                let current = report(&service.status()?);
                if previous.as_ref() != Some(&current) {
                    let (role, peers, lan, remote) = current;
                    eprintln!("uclip: LAN {role:?}, peers {peers}, LAN error {lan:?}, remote error {remote:?}");
                    previous = Some(current);
                }
            }
        }
    }
}
