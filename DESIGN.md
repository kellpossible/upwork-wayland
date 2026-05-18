# upwork-wayland — Design

A single-binary Rust tool that lets Upwork's desktop time-tracker run on Wayland sessions by impersonating the D-Bus interfaces it needs, while routing screenshots through `xdg-desktop-portal` and idle through `ext_idle_notify_v1`.

This document is the agreed contract for the work. It should be updated whenever a decision changes.

## Architecture decision history

This design has shifted twice based on what KWin on Plasma 6.6 actually exposes:

1. **First pass** — assumed KWin supported `zwlr_screencopy_unstable_v1` (the wlroots screencopy protocol used by `grim`). Implemented a full Wayland-thread state machine for it. **Wrong**: KWin has never shipped wlr-screencopy and deliberately won't.
2. **Second pass** — would have switched to `ext_image_copy_capture_v1` (the standardized successor that KDE intends to adopt). **Also wrong for now**: not yet exposed by KWin as a Wayland global as of Plasma 6.6.4 (the user's system).
3. **Current** — routes screenshots through `org.freedesktop.portal.Screenshot` (xdg-desktop-portal). Works on KDE 6.5+ with a one-time consent grant (the "Permissions" page introduced in Plasma 6.5 lets the user pick "Allow" persistently for our service). Idle tracking still uses the native Wayland protocol `ext_idle_notify_v1`.

---

## 1. Background & Motivation

Upwork's Linux desktop application requires an X11 session. On a pure-Wayland desktop it refuses to launch ("Upwork screenshots are only available on Xorg sessions"), and even when forced past that gate it relies on the GNOME-Shell D-Bus screenshot interface to capture screens, plus the Mutter idle-monitor interface to compute idle time.

A prior workaround exists as a small Python project (`upwork-wayland-workaround`, located at `../upwork-wayland-workaround`). It is functional but:

- Depends on **`grim`** (external binary) for screen capture, which limits it to compositors that support `wlr-screencopy` and that have `grim` installed.
- Depends on **`swayidle`** (external binary) for idle tracking, with the same compositor constraints.
- Ships as **two files** (`screenshot.py` D-Bus service + `upwork-launcher.sh` bash launcher).
- Targets **GNOME** explicitly in its docs, even though the grim/swayidle dependencies push it toward wlroots compositors in practice.

We want to replace it with a Rust binary that is self-contained, uses native Wayland protocols directly (no `grim`/`swayidle` spawning), and is structured to allow additional desktop environments to be added later.

## 2. Goals

1. **Single binary, no shell scripts.** `upwork-wayland` is the entry point; launching Upwork and running the D-Bus bridge happen in the same process.
2. **No runtime dependency on external binaries** (`grim`, `swayidle`) for screen capture or idle tracking. The Wayland compositor, `xdg-desktop-portal`, and the session D-Bus daemon are the only external services we rely on.
3. **Primary support for KDE Plasma 6 / KWin Wayland (≥ 6.5).** Screenshots via `xdg-desktop-portal` (works cross-DE as a side effect). Idle via `ext_idle_notify_v1` (KWin Plasma 5.27+).
4. **Backend traits for replaceability.** `ScreenshotBackend` and `IdleBackend` are traits — if KWin ships `ext_image_copy_capture_v1` later, or if we want a native KWin-D-Bus path, that's a new backend impl, not a rewrite.
5. **Personal-tool scope.** No CI, no release pipeline, no packaging for distros. Build with `cargo build --release`, install with `cargo install --path .` plus the bundled `install` subcommand for the `.desktop` entry.
6. **Faithful D-Bus surface.** Emulate exactly the methods Upwork calls on the Python original — no more, no less — so behavior matches.
7. **Lifetime tied to Upwork.** When Upwork exits, the bridge exits.
8. **No async runtime in our own code.** We use `zbus`'s blocking API and `wayland-client`'s synchronous dispatch. Concurrency is plain `std::thread` + channels. (zbus pulls in `async-io` transitively, but that is an implementation detail of zbus, not something we write against.)

## 3. Non-Goals

- Window-specific screenshots (Wayland security model + portal API both prevent capturing arbitrary windows; we fall back to full-screen, like the Python original).
- Rectangle-area screenshots (`org.freedesktop.portal.Screenshot` has no area variant; we fall back to full-screen here too — Upwork doesn't appear to use area captures in practice).
- A general-purpose Wayland-to-GNOME compatibility shim. We implement only what Upwork calls.
- A long-running daemon / system service. The bridge is a per-launch sidecar.
- TOML config file (deferred — env vars + CLI flags are enough for personal use).
- systemd unit (deferred for same reason).
- Structured logging via `tracing` (overkill — we use the `log` facade + `env_logger` backend, controlled by `RUST_LOG`).
- A user-written async runtime. zbus needs one internally; we use its blocking API and avoid having to think about it.
- Continuous capture / PipeWire / `org.freedesktop.portal.ScreenCast`. PipeWire would lower steady-state latency below the Screenshot portal's ~200–400ms but require setting up a video pipeline that produces frames continuously. For Upwork's ~6 captures/hour, that's strictly more machinery for no benefit.

## 4. Requirements

### 4.1 Functional

| ID  | Requirement |
|---  |---          |
| F1  | Implements D-Bus interface `org.gnome.Shell.Screenshot` on the session bus with methods `Screenshot(bbs) → (bs)`, `ScreenshotWindow(bbbs) → (bs)`, `ScreenshotArea(iiiibs) → (bs)`. |
| F2  | Implements D-Bus interface `org.gnome.Mutter.IdleMonitor` on the session bus with method `GetIdletime() → t` returning idle time in **milliseconds**. |
| F3  | Captures screenshots via `org.freedesktop.portal.Screenshot.Screenshot(parent_window, options) → handle`, waits for the matching `org.freedesktop.portal.Request.Response` signal, and moves the resulting PNG to the caller-supplied filename. |
| F4  | Tracks idle time via `ext_idle_notify_v1`, updating a "last active" timestamp on resume events. |
| F5  | Launches the Upwork binary (`/opt/Upwork/upwork` or `/usr/bin/upwork`, configurable via CLI flag) as a child process with `XDG_SESSION_TYPE=x11` set and `WAYLAND_DISPLAY` unset in its environment. |
| F6  | Exits when the Upwork child process exits, propagating its exit code. Cleans up D-Bus name registration and Wayland connection on exit. |
| F7  | An `install` subcommand writes `~/.local/share/applications/upwork-wayland.desktop` with the `Exec=` line pointing at `upwork-wayland run`. The basename must be `upwork-wayland.desktop` to match the app_id we register with the portal. |
| F8  | At portal-backend startup, calls `org.freedesktop.host.portal.Registry.Register("upwork-wayland", {})` so the Plasma 6.5+ permission-grant dialog can show our Name/Icon instead of the generic "An app wants to take screenshots". |

### 4.2 Non-functional

- **Compositor + portal target.** KWin Wayland on KDE Plasma 6.5+ is what we test against. Screenshots also require `xdg-desktop-portal` + `xdg-desktop-portal-kde` to be installed and running (both are standard on Plasma; not a new dependency in practice). On other Wayland desktops (GNOME, wlroots+xdg-desktop-portal-wlr) the screenshot path should work unchanged; the idle path needs whatever compositor supports `ext_idle_notify_v1` (GNOME 45+, KWin 5.27+, recent wlroots).
- **First-call latency.** Plasma 6.5 portal Screenshot: ~200–400ms steady state. The first call after the user grants permission may show a brief consent dialog — after that it goes through silently.
- **Startup latency.** The bridge must be ready (D-Bus names claimed, Wayland globals bound) before Upwork is exec'd, so the first screenshot call doesn't race. ~1s budget.
- **Failure mode.** If we cannot claim the D-Bus name (because GNOME Shell or another bridge already has it) we exit with a clear error before launching Upwork.

## 5. Architecture

### 5.1 Process model

One process. **No async/await in our own code** — but zbus runs its own internal executor and connection-reader threads behind the blocking API, so the real OS-thread inventory at steady state is:

| Thread | Owner | Role |
|---     |---    |---   |
| main | us | Parses CLI, brings everything up, blocks on `child.wait()`, then shuts everything down. |
| Wayland | us | Owns the `wl_display` connection and runs the calloop event loop. |
| signal-handler | us | Short-lived thread iterating `signal_hook::iterator::Signals` over SIGINT/SIGTERM, kills Upwork child on signal. |
| zbus executor | zbus | Drives zbus's internal async tasks. Created automatically by `zbus::blocking::Connection`. We never see it. |
| zbus socket reader | zbus | Reads from the D-Bus socket. Also internal. |

We use zbus's default features, which give us the `async-io`-based runtime plus the `blocking-api`. Per the zbus docs, enabling the `tokio` feature would *"launch no threads behind your back"* by folding zbus's work into a tokio runtime we'd own end-to-end — but that's strictly more complexity for a tool that handles a screenshot every ~10 min, so we keep the default path and accept the 2 zbus-managed threads.

```
   ┌────────────────────────────────────────────────────────────┐
   │ main thread                                                │
   │   1. parse CLI                                             │
   │   2. spawn Wayland thread, wait for it to signal "ready"   │
   │   3. start zbus blocking Connection, register interfaces   │
   │   4. spawn Upwork child (std::process::Command)            │
   │   5. child.wait() — block until Upwork exits               │
   │   6. signal shutdown to Wayland thread, drop zbus conn     │
   │   7. exit with Upwork's exit code                          │
   ├────────────────────────────────────────────────────────────┤
   │ Wayland thread                                             │
   │   - owns the wl_display connection + EventQueue            │
   │   - runs a calloop EventLoop multiplexing:                 │
   │       * WaylandSource (the wl_display fd)                  │
   │       * calloop::ping for the shutdown signal              │
   │   - on ext_idle_notify "resumed" event, stores             │
   │     Instant::now() into Arc<Mutex<Instant>> (last_active)  │
   ├────────────────────────────────────────────────────────────┤
   │ zbus internal thread(s)                                    │
   │   - zbus spawns its own executor; method handlers run      │
   │     synchronously from our perspective                     │
   │   - Screenshot handler: calls PortalBackend.capture() →    │
   │     blocking D-Bus call into xdg-desktop-portal →          │
   │     fs::rename PNG to caller's filename →                  │
   │     return (bool, String)                                  │
   │   - GetIdletime handler: lock last_active, compute delta,  │
   │     return u64 millis                                      │
   └────────────────────────────────────────────────────────────┘
```

Note that no cross-thread channel is needed for screenshots anymore — the portal call is itself a D-Bus call that the zbus handler thread makes directly. The Wayland thread is dedicated to the idle subscription. `last_active` is a `Arc<Mutex<Instant>>` — atomic ops on `Instant` aren't directly possible and a mutex around an `Instant` is cheap and unambiguous.

### 5.1.1 Shutdown flow

There are exactly two shutdown triggers:
- **Upwork exits on its own** — `child.wait()` on the main thread returns.
- **We receive SIGINT / SIGTERM** — the `signal-hook` handler kills the Upwork child PID, which makes `child.wait()` return shortly after.

Both paths converge on the same main-thread sequence:

```
1. child.wait() returns ExitStatus { code }
2. drop(zbus_connection)        // releases org.gnome.Shell.Screenshot
                                //          and org.gnome.Mutter.IdleMonitor
3. wayland_shutdown_tx.send(()) // calloop wakes, breaks out of dispatch loop
4. wayland_thread.join()
5. exit with `code`
```

The signal handler is intentionally tiny: it just calls `kill(child_pid, SIGTERM)` (and if a second signal arrives, `SIGKILL`). It does **not** try to do any cleanup itself — all cleanup happens in the main thread after `wait()` unblocks. This avoids the classic signal-handler pitfalls (no allocations, no locks held across signal boundaries).

If `wait()` doesn't return within a small grace period after we kill the child (~2 seconds), we escalate to `SIGKILL` and try again. Not expected in practice but defensible.

### 5.2 D-Bus method behavior (matching the Python original)

| Method | Behavior |
|---     |---       |
| `Screenshot(include_cursor, flash, filename)` | Full-desktop capture via xdg-desktop-portal. `flash` ignored. `include_cursor` ignored (portal decides). Returns `(success, filename_actually_used)`. |
| `ScreenshotWindow(include_frame, include_cursor, flash, filename)` | Falls back to full-desktop capture (portal cannot screenshot specific windows for an external caller). |
| `ScreenshotArea(x, y, width, height, flash, filename)` | Falls back to full-desktop capture (portal has no area variant). Coordinates ignored. |
| `GetIdletime() → t` | Milliseconds since last user activity. |

Empty filename or missing parent directory: we create the parent directory if needed (mirrors `os.makedirs(..., exist_ok=True)` in the Python). Errors result in `(false, "")`.

### 5.3 Screenshot backend

```rust
trait ScreenshotBackend: Send + Sync {
    /// Capture the full desktop. Returns the path of a PNG file on
    /// $XDG_RUNTIME_DIR that the caller must rename or copy to its target.
    fn capture(&self) -> Result<PathBuf>;
}
```

The v0.1 implementation is `PortalBackend`. At construction it:

- Connects a fresh `zbus::blocking::Connection` to the session bus.
- Calls `org.freedesktop.host.portal.Registry.Register("upwork-wayland", {})` to declare our app identity (see [§5.3.2](#532-app-identification-via-the-registry-portal)). Best-effort — older portals don't expose Registry; we log and continue.

For each capture:

1. Generates a unique `handle_token` and predicts the resulting Request object path (`/org/freedesktop/portal/desktop/request/<sender_escaped>/<token>`) so it can subscribe to the `Response` signal BEFORE making the call, avoiding a race against a very fast portal.
2. Calls `org.freedesktop.portal.Screenshot.Screenshot("", { handle_token, interactive=false })`.
3. Blocks waiting for the `org.freedesktop.portal.Request.Response` signal on the predicted path.
4. Parses `results["uri"]`, percent-decodes the file path, returns it.

The D-Bus method handler then `fs::rename`s (with `fs::copy + fs::remove_file` as a cross-filesystem fallback) the portal's temp file onto the caller's filename. Both are normally on tmpfs (`$XDG_RUNTIME_DIR` and the target path Upwork picks), so rename is atomic.

We do **not** depend on the `png` crate. The portal hands us a finished PNG file; we move it.

### 5.3.2 App identification via the Registry portal

By default, the Plasma 6.5+ permission-grant dialog displays "An app wants to take screenshots" because xdg-desktop-portal can't infer our identity for an unsandboxed binary launched outside a systemd-cgroup `app-*` scope.

The standardized fix (xdg-desktop-portal ≥ 1.18) is the `org.freedesktop.host.portal.Registry.Register(app_id, options)` method, called once per D-Bus connection before any other portal call. We use `app_id = "upwork-wayland"`.

For the portal to resolve that app_id to a display name and icon, a `.desktop` file with the matching basename (`upwork-wayland.desktop`) must be installed in `$XDG_DATA_DIRS/applications/` or `~/.local/share/applications/`. Our `install` subcommand writes this file. Until it's installed, the consent dialog falls back to the generic phrase even though we registered.

### 5.3.1 Why not other paths

| Considered | Why rejected for now |
|---         |---                   |
| `zwlr_screencopy_unstable_v1` | KWin does not implement it (and won't). Works on wlroots compositors. Would require maintaining a Wayland-thread state machine, shm buffers, PNG encoding. |
| `ext_image_copy_capture_v1` | The successor that KDE plans to adopt. **Not yet** advertised by KWin Plasma 6.6.4 (verified by `wayland-info` on the target system). Reconsider when KDE ships it. |
| `org.kde.KWin.ScreenShot2` | KDE-native, no consent dialog, ~5× faster. But KWin enforces a D-Bus peer allowlist; arbitrary clients get an authorization error. The workaround (`KWIN_SCREENSHOT_NO_PERMISSION_CHECKS=1` in the KWin session env) is global, invasive, and requires manual user setup. |
| `org.freedesktop.portal.ScreenCast` + PipeWire | Designed for video; setup cost (`CreateSession` + `SelectSources` + `Start` + PipeWire negotiation) dwarfs a single capture. Worth it for ≥1 Hz capture, pointless for 6/hour. |

### 5.4 Idle backend

```rust
trait IdleBackend: Send + Sync {
    /// Returns milliseconds since last user activity.
    fn idle_time_ms(&self) -> u64;
}
```

The single v0.1 implementation is `ExtIdleNotifyBackend`. On startup we create an `ext_idle_notifier_v1.get_idle_notification(timeout, seat)` with a small timeout (e.g. 1000ms, matching `swayidle -w 'timeout' '1'` in the Python). On `resumed` events we record `Instant::now()` into a shared atomic. `idle_time_ms()` returns `now - last_active` in milliseconds.

> Limitation inherited from the original: between 0 and `timeout` ms after activity we may report stale idle time. Upwork polls infrequently so this is acceptable.

### 5.5 Launcher

`launcher::find_upwork()` checks, in order:

1. `--upwork-path` CLI flag if given
2. `$UPWORK_BINARY` env var
3. `/opt/Upwork/upwork`
4. `/usr/bin/upwork`

`launcher::spawn(path)` uses `std::process::Command` with:

```rust
.env("XDG_SESSION_TYPE", "x11")
.env_remove("WAYLAND_DISPLAY")
.spawn()?
```

We call `child.wait()` from the main thread and use the resulting exit code as our own. The `std` `Command` doesn't have `kill_on_drop`, so on a panic / signal we explicitly kill the child via a `Drop` guard wrapper (small type owning `Child` that calls `kill()` if dropped without `into_inner()`).

### 5.6 `install` subcommand

Generates `~/.local/share/applications/upwork.desktop` from a baked-in template, substituting the absolute path of the current `upwork-wayland` binary (via `std::env::current_exe()`). Refuses to overwrite an existing file unless `--force` is passed.

## 6. CLI

```
upwork-wayland [run] [--upwork-path PATH] [-v|--verbose]
upwork-wayland install [--force]
upwork-wayland --help
upwork-wayland --version
```

`run` is the default subcommand if none is given.

## 7. Project layout

```
upwork-wayland/
├── Cargo.toml
├── mise.toml
├── mise.lock
├── DESIGN.md
├── README.md         (later)
├── LICENSE           (MIT, matches original)
└── src/
    ├── main.rs       — entry, env_logger init, subcommand dispatch
    ├── cli.rs        — clap definitions
    ├── bridge/
    │   ├── mod.rs    — wire-up: spawn Wayland thread, build backends, claim D-Bus names
    │   ├── dbus.rs   — zbus interface impls for Screenshot & IdleMonitor; fs::rename helper
    │   ├── wayland.rs— Wayland connection + calloop loop; just the idle subscription
    │   ├── screenshot/
    │   │   ├── mod.rs   — ScreenshotBackend trait
    │   │   └── portal.rs— org.freedesktop.portal.Screenshot client
    │   └── idle/
    │       ├── mod.rs— IdleBackend trait
    │       └── ext.rs— reads last_active updated by the Wayland thread
    ├── launcher.rs   — find Upwork, spawn with X11 env, signal-hook integration, kill-on-drop guard
    └── install.rs    — write .desktop file (with diff-style overwrite confirmation)
```

## 8. Dependencies

Researched May 2026. All chosen for active maintenance, strong adoption, and tight scope fit. We pin minor versions in `Cargo.toml` and rely on `Cargo.lock` for reproducibility.

| Crate | Version | Why |
|---    |---      |---  |
| `zbus` | 5.15 | De-facto pure-Rust D-Bus crate. `#[interface]` macro for server-side; `blocking::Proxy` for the portal client call. We use its **blocking API** so we don't write any async code. zbus uses `async-io` internally; that's transitive only. |
| `wayland-client` | 0.31 | Smithay's low-level client. We drive it through `calloop` rather than `EventQueue::blocking_dispatch` so the Wayland thread can also wake on shutdown. |
| `wayland-protocols` | 0.32 | Provides `ext_idle_notify_v1` (under the staging module). |
| `calloop` | 0.14 | Smithay's event loop, used for the Wayland-thread loop. Multiplexes the Wayland connection fd with a `calloop::ping` source for shutdown signaling. |
| `calloop-wayland-source` | 0.4 | Adapter that exposes the Wayland `EventQueue` as a calloop event source. |
| `signal-hook` | 0.3 | Async-signal-safe handler for SIGINT/SIGTERM. Dedicated thread iterating `Signals` forwards SIGTERM to the Upwork child PID; main thread does the rest after `child.wait()` unblocks. |
| `clap` | 4.6 | Standard CLI parser. `derive` feature. |
| `anyhow` | 1.0 | Application-level error handling. We don't expose a library so `thiserror` is not warranted. |
| `log` | 0.4 | Logging facade. |
| `env_logger` | 0.11 | Tiny console backend for the `log` facade, controlled by `RUST_LOG=info` (etc.). Writes to stderr. |
| `rustix` | 1 | `kill_process` for sending SIGTERM to the Upwork child PID from the signal-handler thread. `process` feature only — no `fs`/`mm` needed now that we don't allocate shm. |

### 8.1 Considered and rejected

- **`tokio`** — would force us to write async glue everywhere. zbus has a perfectly good blocking API; the rest of our code maps naturally to plain threads.
- **`tracing` + `tracing-subscriber`** — better than `log`/`env_logger` for non-trivial apps but overkill for a single-process personal tool. Easy to migrate later because we go through the `log` facade.
- **`dbus` (libdbus binding)** — pulls in C `libdbus`, sync-first API. zbus is strictly better for our case.
- **`smithay-client-toolkit`** — designed for GUI clients. Raw `wayland-client` is fine.
- **`image`** — we no longer encode PNGs at all (portal returns one). Removed.
- **`png`** — same reason, removed.
- **`wayland-protocols-wlr`** — would only be needed for wlr-screencopy; KWin doesn't support it, so removed.
- **`memmap2`** — was only needed for shm buffer transfer in the wlr-screencopy path. Removed with that path.
- **`ashpd`** (Rust portal wrapper) — async-first, would require committing to an async runtime we don't otherwise want. The portal Screenshot call is ~140 lines of direct zbus, which is acceptable.
- **`ctrlc`** — simpler than `signal-hook` but only handles Ctrl-C; `signal-hook` is barely heavier and gives us SIGTERM cleanly.
- **`thiserror`** — only worth adding if we expose typed errors across module/library boundaries; we don't.
- **`pico-args`/`lexopt`** — smaller than clap, but clap's derive ergonomics are worth the binary-size cost for a desktop tool.
- **Manual `poll(2)` on the Wayland fd** — viable, but `calloop` exists and is the documented pattern for multiplexing Wayland with other event sources.

### 8.2 Built-it-ourselves check

- **D-Bus server + client** — not viable (wire format + auth handshake). Use `zbus`.
- **Wayland client** — viable in theory but ~3000 lines of XML-driven boilerplate. Use the Smithay stack.
- **Percent-decoding for portal's file:// URI** — viable and trivial (~15 lines). We do it inline rather than pulling in `percent-encoding` or `url`.
- **`.desktop` file writing** — trivial, do it inline (no `freedesktop-desktop-entry` crate needed).
- **Upwork binary path lookup** — trivial, inline.

## 9. Build & toolchain — `mise`

The project is managed with [mise](https://mise.jdx.dev/). The Rust toolchain version is declared in `mise.toml`; `mise.lock` pins the exact resolved version.

```toml
# mise.toml
[tools]
rust = "1.95.0"
```

Targets and components beyond `rustc`+`cargo` aren't needed at this stage (no cross-compile, no extra rustup components). If we later want `clippy` and `rustfmt` they're available with stock rust toolchains via `rustup component add` and don't need to be listed.

## 10. Testing strategy

Personal-tool scope; we keep this light:

- **Unit tests** for `launcher::find_upwork` (mocked filesystem), `install` template substitution, `portal::percent_decode` and `portal::uri_to_path`.
- **Manual smoke test** documented in README: `cargo run -- run`, then call `gdbus call --session --dest org.gnome.Shell.Screenshot --object-path /org/gnome/Shell/Screenshot --method org.gnome.Shell.Screenshot.Screenshot true false /tmp/test.png` from another terminal; verify the PNG.
- **No integration tests** against a live compositor in CI (no CI). Correctness verified by hand on KDE Plasma 6.6.4.

## 11. Open questions / deferred decisions

1. **Will we ever want the bridge to outlive Upwork?** Decided no for v0.1. If startup latency becomes noticeable, reconsider.
2. **Native ext-image-copy-capture-v1 backend.** When KWin starts advertising that global (likely Plasma 6.7+), adding a `NativeCaptureBackend` as a sibling to `PortalBackend` and preferring it when available would give us no-consent capture and lower latency. Trait already in place.
3. **Multi-output behavior** is moot for the portal path (portal captures the whole desktop). It would re-emerge if we add a native Wayland backend.

## 12. Risks

| Risk | Mitigation |
|---   |---         |
| KWin removes or restricts the portal Screenshot interface. | Trait-based backend layout lets us add native KWin-D-Bus or ext-image-copy-capture backends without touching the D-Bus surface. |
| D-Bus name `org.gnome.Shell.Screenshot` is already claimed (e.g. on GNOME session, by gnome-shell itself). | We detect this at startup, exit with a clear message before launching Upwork. |
| Upwork changes which D-Bus methods it calls. | Logging at every D-Bus method entry will make this immediately visible. We add methods as needed. |
| Portal consent expires or the "Allow" permission gets reset by an update. | First-call failure mode is "user cancelled" — we log it clearly. User re-grants in the consent dialog. |
| `org.freedesktop.host.portal.Registry` is unavailable (xdg-desktop-portal < 1.18). | `Register()` fails non-fatally; we log a warning and proceed without app identification (consent dialog stays generic). |
| User hasn't run `upwork-wayland install`, so no `.desktop` file matches our `app_id`. | Permission dialog stays generic. README/`install` help text directs the user to install before running. |
| The `wayland-protocols` staging module reorganizes `ext_idle_notify_v1`. | Pin to a specific minor version; bump deliberately. |

---

*This DESIGN.md is the contract. Update it when decisions change; don't let it rot.*
