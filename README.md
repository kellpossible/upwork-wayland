# upwork-wayland

Run Upwork's desktop time tracker on a Wayland session.

Upwork refuses to launch on a pure-Wayland desktop ("Upwork screenshots are only available on Xorg sessions") and even when coaxed past that gate, it tries to take screenshots via D-Bus interfaces that only GNOME Shell on X11 implements. This tool stands those interfaces up itself, routes the screenshots through `xdg-desktop-portal`, and launches Upwork in the same process.

Tested on **KDE Plasma 6.6 (Wayland)** on Fedora 44. Should also work on GNOME and wlroots-based compositors with an xdg-desktop-portal backend installed, though that path is untested.

**WARNING:** This app was mostly vibe-coded, tested but not extensively reviewed by a human, use at your own peril!

## What it does

- Claims `org.gnome.Shell.Screenshot` and `org.gnome.Mutter.IdleMonitor` on the session bus, implementing exactly the methods Upwork calls.
- Routes `Screenshot` calls into `org.freedesktop.portal.Screenshot` (xdg-desktop-portal).
- Tracks idle time via the native Wayland `ext_idle_notify_v1` protocol.
- Launches Upwork as a child process with `XDG_SESSION_TYPE=x11` (and `WAYLAND_DISPLAY` unset) so it stops complaining about the session type.
- Exits when Upwork exits; if it's killed unexpectedly, the Linux kernel terminates Upwork too (`PR_SET_PDEATHSIG`).

There is no daemon, no system service, no shell script. One binary, one process.

## Requirements

- A Wayland compositor with `ext_idle_notify_v1` (KWin 5.27+, GNOME 45+, recent wlroots).
- `xdg-desktop-portal` 1.18+ (for the `org.freedesktop.host.portal.Registry` interface, so KDE's permission dialog shows our app name).
- On KDE: **Plasma 6.5+** for persistent per-app screenshot permissions. Earlier versions prompt for consent on every call, which doesn't fit Upwork's polling pattern.
- The Upwork Linux client installed at `/opt/Upwork/upwork` or `/usr/bin/upwork`.
- Rust 1.95 (handled automatically via [mise](https://mise.jdx.dev/)).

## Build

```bash
mise install      # installs the pinned Rust toolchain
mise run build    # cargo build --release
```

## Install

```bash
./target/release/upwork-wayland install
```

That writes `~/.local/share/applications/upwork-wayland.desktop` pointing at the release binary, prompting first if a previous one exists (shows the diff before overwriting).

The desktop file matters for more than the application menu: KDE's permission dialog looks up our app ID (`upwork-wayland`) in the installed `.desktop` files to figure out what name and icon to show on the "Allow screenshots?" prompt. Without it, the dialog falls back to a generic "An app wants to take screenshots".

## Usage

Either launch from the KDE/GNOME application menu after `install`, or directly:

```bash
upwork-wayland run
```

On first launch, KDE will pop up a permission dialog asking whether to allow screenshots. Click **Allow**. The grant persists; subsequent launches go through silently.

### Logging

Default log level is `info`, controlled by `RUST_LOG`:

```bash
RUST_LOG=warn upwork-wayland run     # only warnings / errors
RUST_LOG=debug upwork-wayland run    # verbose, including idle events
RUST_LOG=info,zbus=debug upwork-wayland run    # zbus internals (chatty)
```

zbus and tracing internals are clamped to `warn` regardless of the level you set, unless you name them explicitly as in the third example.

## Tests

```bash
mise run test     # unit + integration tests (skips the live-session smoke test)
mise run smoke    # round-trip smoke test (needs a real Wayland session + portal consent)
```

## Similar projects

- [**muhammad-rafey/upwork-wayland-workaround**](https://github.com/muhammad-rafey/upwork-wayland-workaround) — Python D-Bus bridge using `grim` + `swayidle` subprocesses, plus a bash launcher. Targets GNOME on Wayland; works on wlroots compositors where `grim` is supported. Doesn't work on KWin (which exposes neither `wlr-screencopy` nor a compatible portal-free path). This project is a Rust rewrite of the same idea, using xdg-desktop-portal in place of `grim` and ext-idle-notify in place of `swayidle`.
- [**MarSoft/upwork-wayland**](https://github.com/MarSoft/upwork-wayland) — Earlier Python implementation with the same `grim` + `swayidle` approach. Same wlroots-only limitation.

See [DESIGN.md](./DESIGN.md) for the full architecture story, including the dead ends along the way.

## Troubleshooting

**Permission dialog says "An app wants to take screenshots" with no app name.**
Run `upwork-wayland install` (you may need to do this from the release binary path, not via `cargo run`). The portal resolves the app name from the matching `.desktop` file.

**"binding ext_idle_notifier_v1 ... the requested global was not found in the registry"**
Your compositor doesn't advertise `ext_idle_notify_v1`. Plasma 5.27+ does. Older versions or non-standard compositor builds may not.

**"requesting name org.gnome.Shell.Screenshot ... NameExists"**
Something else is already serving that interface on the session bus. The most likely culprit is a GNOME Shell session — this tool is only useful on non-GNOME Wayland sessions where nothing else claims that name.

**Upwork still complains about the session type.**
Make sure you're launching via `upwork-wayland run` (or the installed `.desktop` entry), not the Upwork binary directly. The env-var override happens in this process.
