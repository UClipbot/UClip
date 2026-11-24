mod clipboard;
mod history;
mod i18n;
mod library;
mod server;

use std::{io, path::PathBuf, process::ExitCode};

use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use uclip_core::{AppError, ErrorCode};

use i18n::{messages, text};

#[derive(Parser)]
#[command(
    name = "uclip",
    version,
    about = text(messages::ABOUT),
    after_help = text(messages::LANGUAGE_OVERRIDE)
)]
struct Cli {
    #[arg(help = text(messages::DATA_DIR), long, global = true, conflicts_with_all = ["dev", "portable"])]
    data_dir: Option<PathBuf>,
    #[arg(help = text(messages::DEV), long, global = true, conflicts_with = "portable")]
    dev: bool,
    #[arg(help = text(messages::PORTABLE), long, global = true)]
    portable: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = text(messages::GET))]
    Get(history::Get),
    #[command(about = text(messages::LIST))]
    List(history::Listing),
    #[command(about = text(messages::SEARCH))]
    Search {
        #[arg(help = text(messages::QUERY))]
        query: String,
        #[arg(long, help = text(messages::REGEX))]
        regex: bool,
        #[command(flatten)]
        listing: history::Listing,
    },
    #[command(about = text(messages::COPY))]
    Copy(history::Copy),
    #[command(about = text(messages::SAVE))]
    Save(history::Save),
    #[command(about = text(messages::COMPLETION))]
    Completion {
        #[arg(help = text(messages::SHELL))]
        shell: CompletionShell,
    },
    #[command(about = text(messages::SYNC))]
    Sync {
        #[command(subcommand)]
        command: server::SyncCommand,
    },
    #[command(about = text(messages::SERVE))]
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
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = i18n::print_parse_error(&error);
            return ExitCode::from(if error.use_stderr() { 2 } else { 0 });
        }
    };
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            i18n::app_error(&error);
            ExitCode::from(match error.code {
                ErrorCode::InvalidInput | ErrorCode::InvalidPattern => 2,
                ErrorCode::NotFound => 3,
                ErrorCode::Busy | ErrorCode::Conflict => 4,
                _ => 1,
            })
        }
    }
}
