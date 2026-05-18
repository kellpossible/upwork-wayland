use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "upwork-wayland", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Launch Upwork with the D-Bus bridge running in the same process (default).
    Run(RunArgs),
    /// Install a .desktop entry pointing at this binary.
    Install(InstallArgs),
}

impl Default for Command {
    fn default() -> Self {
        Command::Run(RunArgs::default())
    }
}

#[derive(clap::Args, Default)]
pub struct RunArgs {
    /// Path to the Upwork binary. Falls back to $UPWORK_BINARY, /opt/Upwork/upwork, /usr/bin/upwork.
    #[arg(long)]
    pub upwork_path: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    /// Skip the confirmation prompt and overwrite without asking.
    #[arg(long)]
    pub force: bool,
}
