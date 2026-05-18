use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;

use anyhow::{Context, Result, bail};
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// Locate the Upwork binary. Order: explicit CLI flag, $UPWORK_BINARY, well-known paths.
pub fn find_upwork(cli_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = cli_path {
        if !p.exists() {
            bail!("--upwork-path {} does not exist", p.display());
        }
        return Ok(p.to_path_buf());
    }
    if let Ok(env) = std::env::var("UPWORK_BINARY") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Ok(p);
        }
    }
    for candidate in ["/opt/Upwork/upwork", "/usr/bin/upwork"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Ok(p);
        }
    }
    bail!("Upwork binary not found in /opt/Upwork/upwork or /usr/bin/upwork — pass --upwork-path");
}

/// Drop-guard: if dropped without `into_inner()`, kills the child.
struct KillOnDrop(Option<Child>);

impl KillOnDrop {
    fn into_inner(mut self) -> Child {
        self.0.take().expect("child already taken")
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            log::warn!("KillOnDrop firing — killing Upwork pid {}", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Spawn Upwork with X11 env, install signal handlers, wait for it to exit,
/// return its exit code.
pub fn spawn_and_wait(path: &Path) -> Result<i32> {
    let mut cmd = Command::new(path);
    cmd.env("XDG_SESSION_TYPE", "x11")
        .env_remove("WAYLAND_DISPLAY");

    // Linux: ask the kernel to send SIGTERM to the child if the main thread
    // of upwork-wayland dies (segfault, SIGKILL, panic, whatever) — without
    // this, Upwork outlives us in those cases.
    unsafe {
        cmd.pre_exec(|| {
            rustix::process::set_parent_process_death_signal(Some(
                rustix::process::Signal::TERM,
            ))
            .map_err(std::io::Error::from)
        });
    }

    let child = cmd
        .spawn()
        .with_context(|| format!("spawning {}", path.display()))?;

    let pid = child.id();
    log::info!("Launched Upwork pid {pid}");
    let guard = KillOnDrop(Some(child));

    // Install a signal-hook thread that translates SIGINT/SIGTERM on us into
    // SIGTERM on the Upwork child. Once the child dies, child.wait() returns
    // and the main thread proceeds with cleanup.
    let mut signals = Signals::new([SIGINT, SIGTERM]).context("installing signal handlers")?;
    let signal_thread = thread::Builder::new()
        .name("signal-handler".into())
        .spawn(move || {
            if let Some(sig) = signals.forever().next() {
                log::warn!("received signal {sig}, forwarding SIGTERM to Upwork pid {pid}");
                let _ = send_signal(pid, rustix::process::Signal::TERM);
            }
        })
        .context("spawning signal handler thread")?;

    let status = guard.into_inner().wait().context("waiting on Upwork")?;
    log::info!("Upwork exited: {status}");

    // The signal thread will be parked in signals.forever() waiting for a
    // signal that may never come. Detach it — the process is about to exit
    // anyway.
    drop(signal_thread);

    Ok(status.code().unwrap_or(0))
}

fn send_signal(pid: u32, sig: rustix::process::Signal) -> Result<()> {
    let pid =
        rustix::process::Pid::from_raw(pid as i32).context("converting pid")?;
    rustix::process::kill_process(pid, sig).context("kill_process")?;
    Ok(())
}
