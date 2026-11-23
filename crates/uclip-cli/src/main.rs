mod clipboard;
mod history;
mod library;
mod server;

use std::{io, path::PathBuf, process::ExitCode};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use uclip_core::{AppError, ErrorCode};

#[derive(Parser)]
#[command(
    name = "uclip",
    version,
    about = "UClip clipboard history and synchronization"
)]
struct Cli {
    /// Explicit UClip library directory (also supported via UCLIP_DATA_DIR).
    #[arg(long, global = true, conflicts_with_all = ["dev", "portable"])]
    data_dir: Option<PathBuf>,
    /// Use the library of npm run tauri:dev.
    #[arg(long, global = true, conflicts_with = "portable")]
    dev: bool,
    /// Use uclip-data beside this executable.
    #[arg(long, global = true)]
    portable: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read a record by UUID, or the newest record in history.
    Get(history::Get),
    /// List active history in desktop order.
    List(history::Listing),
    /// Search text in active history.
    Search {
        query: String,
        #[arg(long)]
        regex: bool,
        #[command(flatten)]
        listing: history::Listing,
    },
    /// Write to the system clipboard.
    Copy(history::Copy),
    /// Save text, an image, or explicit file references.
    Save(history::Save),
    /// Generate shell completions.
    Completion { shell: CompletionShell },
    /// Configure a synchronization group.
    Sync {
        #[command(subcommand)]
        command: server::SyncCommand,
    },
    /// Run encrypted synchronization without a desktop.
    Serve,
    #[cfg(target_os = "linux")]
    #[command(name = "__clipboard-owner", hide = true)]
    ClipboardOwner,
}

#[derive(Clone, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
}

fn invalid() -> AppError {
    AppError::new(ErrorCode::InvalidInput)
}

fn unavailable() -> AppError {
    AppError::new(ErrorCode::Unavailable)
}

fn storage() -> AppError {
    AppError::new(ErrorCode::Storage)
}

fn print_json(value: &impl serde::Serialize) -> Result<(), AppError> {
    use std::io::Write;
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, value).map_err(|_| unavailable())?;
    writeln!(out).map_err(|_| unavailable())
}

fn run(cli: Cli) -> Result<(), AppError> {
    match cli.command {
        Command::Completion { shell } => {
            let shell = match shell {
                CompletionShell::Bash => clap_complete::Shell::Bash,
                CompletionShell::Zsh => clap_complete::Shell::Zsh,
            };
            clap_complete::generate(shell, &mut Cli::command(), "uclip", &mut io::stdout());
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        Command::ClipboardOwner => return clipboard::hold_selection(),
        Command::Copy(args) if args.id.is_none() && !args.save => {
            return clipboard::publish(args.input.read()?);
        }
        _ => {}
    }

    let location = library::Location::resolve(cli.data_dir, cli.dev, cli.portable)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| unavailable())?;
    runtime.block_on(async {
        let create = matches!(
            &cli.command,
            Command::Save(_)
                | Command::Copy(history::Copy { save: true, .. })
                | Command::Sync {
                    command: server::SyncCommand::Create | server::SyncCommand::Join
                }
        );
        let core = location.open(create).await?;
        let result = match cli.command {
            Command::Get(args) => history::get(&core, args).await,
            Command::List(args) => history::list(&core, args, String::new(), false, None).await,
            Command::Search {
                query,
                regex,
                listing,
            } => {
                history::list(
                    &core,
                    listing,
                    query,
                    regex,
                    Some(uclip_core::ClipKind::Text),
                )
                .await
            }
            Command::Copy(args) => history::copy(&core, args).await,
            Command::Save(args) => history::save(&core, args).await,
            Command::Sync { command } => server::configure(&core, &location.path, command).await,
            Command::Serve => return server::serve(&core, &location.path).await,
            Command::Completion { .. } => unreachable!(),
            #[cfg(target_os = "linux")]
            Command::ClipboardOwner => unreachable!(),
        };
        let closed = core.shutdown().await;
        result.and(closed)
    })
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = serde_json::to_value(error.code).ok();
            let name = code
                .as_ref()
                .and_then(|code| code.as_str())
                .unwrap_or("unavailable");
            eprintln!("uclip: {name}");
            ExitCode::from(match error.code {
                ErrorCode::InvalidInput | ErrorCode::InvalidPattern => 2,
                ErrorCode::NotFound => 3,
                ErrorCode::Busy | ErrorCode::Conflict => 4,
                _ => 1,
            })
        }
    }
}
