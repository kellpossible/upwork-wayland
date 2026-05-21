use anyhow::Result;
use clap::Parser;

mod bridge;
mod cli;
mod install;
mod launcher;

fn main() -> Result<()> {
    init_logging();
    let cli = cli::Cli::parse();
    match cli.command.unwrap_or_default() {
        cli::Command::Run(args) => run(args),
        cli::Command::Serve => serve(),
        cli::Command::Install(args) => install::run(args),
    }
}

fn serve() -> Result<()> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::iterator::Signals;

    let bridge = bridge::start()?;
    log::info!("bridge running; send SIGINT/SIGTERM to stop");
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    if let Some(sig) = signals.forever().next() {
        log::info!("received signal {sig}, shutting down");
    }
    bridge.shutdown();
    Ok(())
}

/// Configure `env_logger` from `RUST_LOG` (defaulting to `info` if unset),
/// then forcibly clamp the chatty dependencies (`zbus`, `tracing`) down to
/// `warn`. Users who actually want zbus internals can override with e.g.
/// `RUST_LOG=info,zbus=debug`; the last matching directive wins.
fn init_logging() {
    let user_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let mut directives = vec!["zbus=warn", "tracing=warn"];
    // Only append our defaults if the user hasn't already named that module —
    // that way `RUST_LOG=info,zbus=debug` keeps the user's choice.
    let user_modules: Vec<&str> = user_filter
        .split(',')
        .filter_map(|s| s.split_once('=').map(|(m, _)| m.trim()))
        .collect();
    directives.retain(|d| {
        let module = d.split_once('=').map(|(m, _)| m).unwrap_or("");
        !user_modules.contains(&module)
    });
    let final_filter = if directives.is_empty() {
        user_filter
    } else {
        format!("{user_filter},{}", directives.join(","))
    };
    let _ = env_logger::Builder::new()
        .parse_filters(&final_filter)
        .try_init();
}

fn run(args: cli::RunArgs) -> Result<()> {
    let upwork_path = launcher::find_upwork(args.upwork_path.as_deref())?;
    log::info!("Found Upwork at {}", upwork_path.display());

    let bridge = bridge::start()?;
    let exit_code = launcher::spawn_and_wait(&upwork_path)?;
    bridge.shutdown();

    std::process::exit(exit_code);
}
