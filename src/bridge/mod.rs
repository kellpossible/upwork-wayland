//! D-Bus bridge: claims org.gnome.Shell.Screenshot and org.gnome.Mutter.IdleMonitor,
//! routes screenshot calls through xdg-desktop-portal and idle queries through
//! the Wayland-thread `ext_idle_notify_v1` subscription.

use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{Context, Result};

pub mod dbus;
pub mod idle;
pub mod screenshot;
pub mod wayland;

use idle::ext::ExtIdleBackend;
use screenshot::portal::PortalBackend;

pub struct Bridge {
    // Holding the connection keeps the D-Bus names claimed; dropping releases them.
    _connection: zbus::blocking::Connection,
    shutdown_ping: calloop::ping::Ping,
    thread: Option<JoinHandle<Result<()>>>,
}

impl Bridge {
    pub fn shutdown(mut self) {
        self.shutdown_ping.ping();
        if let Some(t) = self.thread.take() {
            match t.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => log::warn!("wayland thread returned error: {e:#}"),
                Err(_) => log::warn!("wayland thread panicked"),
            }
        }
    }
}

pub fn start() -> Result<Bridge> {
    let wayland::Handles {
        last_active,
        shutdown,
        thread,
    } = wayland::start().context("starting Wayland thread")?;

    let screenshot_backend: Arc<dyn screenshot::ScreenshotBackend> =
        Arc::new(PortalBackend::new().context("initializing portal screenshot backend")?);
    let idle_backend: Arc<dyn idle::IdleBackend> = Arc::new(ExtIdleBackend::new(last_active));

    let screenshot_service = dbus::ScreenshotService::new(screenshot_backend);
    let idle_service = dbus::IdleMonitorService::new(idle_backend);

    let connection = zbus::blocking::connection::Builder::session()
        .context("connecting to session bus")?
        .name(dbus::SCREENSHOT_BUS_NAME)
        .with_context(|| format!("requesting name {}", dbus::SCREENSHOT_BUS_NAME))?
        .name(dbus::IDLE_BUS_NAME)
        .with_context(|| format!("requesting name {}", dbus::IDLE_BUS_NAME))?
        .serve_at(dbus::SCREENSHOT_PATH, screenshot_service)
        .with_context(|| format!("serving at {}", dbus::SCREENSHOT_PATH))?
        .serve_at(dbus::IDLE_PATH, idle_service)
        .with_context(|| format!("serving at {}", dbus::IDLE_PATH))?
        .build()
        .context("building zbus connection")?;

    log::info!(
        "claimed {} and {} on session bus",
        dbus::SCREENSHOT_BUS_NAME,
        dbus::IDLE_BUS_NAME
    );

    Ok(Bridge {
        _connection: connection,
        shutdown_ping: shutdown,
        thread: Some(thread),
    })
}
