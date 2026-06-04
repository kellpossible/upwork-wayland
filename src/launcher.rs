use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

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
    // Two-part trick to make *periodic* screenshots work on Wayland. Upwork's
    // uta_native.node picks its capture path from a native Wayland probe
    // (`define_session_type`: dlopen libwayland-client + `wl_display_connect`):
    //
    //   * isWayland = true  → capture via D-Bus screenshot interface, BUT a
    //     separate JS guard (`if (utaNative.isWayland()) return <empty>`) fires
    //     first and blocks every periodic capture — only the first lands.
    //   * isWayland = false → capture in-process via `gdk_pixbuf_get_from_window`
    //     on the X11 root window, which is all-black under XWayland.
    //
    // Both read the same detection byte, so no env var satisfies both. We force
    // isWayland = false (so the JS guard stops blocking) and LD_PRELOAD a shim
    // that interposes `gdk_pixbuf_get_from_window`, returning a real frame
    // captured through our bridge instead of the black root window.
    //
    // WAYLAND_DISPLAY="" (not unset) makes the probe report false while staying
    // falsy in JS: `wl_display_connect` with an empty socket name fails to
    // connect, whereas *unset* makes libwayland fall back to the default
    // `wayland-0` and connect. Verified against the real libwayland.
    cmd.env("XDG_SESSION_TYPE", "x11")
        .env("WAYLAND_DISPLAY", "");

    let _shim = match install_capture_shim() {
        Ok((dir, so_path)) => {
            cmd.env("LD_PRELOAD", &so_path);
            Some(CaptureShimGuard(Some(dir)))
        }
        Err(e) => {
            log::warn!(
                "could not install capture shim ({e:#}); periodic screenshots \
                 will be black"
            );
            None
        }
    };

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

/// The capture shim cdylib, compiled from `shim/gdk_shim.rs` by `build.rs` and
/// embedded here. Written to a temp file and `LD_PRELOAD`ed into Upwork.
const CAPTURE_SHIM_SO: &[u8] = include_bytes!(env!("UPWORK_GDK_SHIM"));

/// Materialise the embedded capture shim into a fresh temp dir and return that
/// dir plus the path to the `.so` for `LD_PRELOAD`.
fn install_capture_shim() -> Result<(PathBuf, PathBuf)> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "upwork-wayland-shim-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir(&dir).with_context(|| format!("creating shim dir {}", dir.display()))?;

    let so = dir.join("libupwork_gdk_shim.so");
    fs::write(&so, CAPTURE_SHIM_SO).with_context(|| format!("writing shim {}", so.display()))?;
    fs::set_permissions(&so, fs::Permissions::from_mode(0o755))
        .with_context(|| format!("chmod 755 {}", so.display()))?;

    log::info!("installed capture shim at {}", so.display());
    Ok((dir, so))
}

/// Drop-guard: removes the shim temp dir on exit.
struct CaptureShimGuard(Option<PathBuf>);

impl Drop for CaptureShimGuard {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take()
            && let Err(e) = fs::remove_dir_all(&dir)
        {
            log::debug!("removing shim dir {}: {e}", dir.display());
        }
    }
}

