//! zbus interface implementations for org.gnome.Shell.Screenshot
//! and org.gnome.Mutter.IdleMonitor.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use zbus::interface;

use super::idle::IdleBackend;
use super::screenshot::ScreenshotBackend;

pub const SCREENSHOT_BUS_NAME: &str = "org.gnome.Shell.Screenshot";
pub const SCREENSHOT_PATH: &str = "/org/gnome/Shell/Screenshot";

pub const IDLE_BUS_NAME: &str = "org.gnome.Mutter.IdleMonitor";
pub const IDLE_PATH: &str = "/org/gnome/Mutter/IdleMonitor/Core";

pub struct ScreenshotService {
    backend: Arc<dyn ScreenshotBackend>,
}

impl ScreenshotService {
    pub fn new(backend: Arc<dyn ScreenshotBackend>) -> Self {
        Self { backend }
    }

    fn do_capture(&self, method: &str, filename: &str) -> (bool, String) {
        if filename.is_empty() {
            log::error!("{method}: empty filename");
            return (false, String::new());
        }
        let target = PathBuf::from(filename);

        let captured = match self.backend.capture() {
            Ok(p) => p,
            Err(e) => {
                log::error!("{method}: capture failed: {e:#}");
                return (false, String::new());
            }
        };

        if let Err(e) = place(&captured, &target) {
            log::error!("{method}: placing captured PNG at {target:?} failed: {e:#}");
            return (false, String::new());
        }

        log::info!("{method} → {}", target.display());
        (true, target.to_string_lossy().into_owned())
    }
}

#[interface(name = "org.gnome.Shell.Screenshot")]
impl ScreenshotService {
    fn screenshot(
        &self,
        include_cursor: bool,
        _flash: bool,
        filename: String,
    ) -> (bool, String) {
        log::debug!("Screenshot({include_cursor}, _, {filename:?})");
        self.do_capture("Screenshot", &filename)
    }

    #[zbus(name = "ScreenshotWindow")]
    fn screenshot_window(
        &self,
        _include_frame: bool,
        include_cursor: bool,
        _flash: bool,
        filename: String,
    ) -> (bool, String) {
        log::debug!(
            "ScreenshotWindow(.., {include_cursor}, _, {filename:?}) [→ full-screen]"
        );
        self.do_capture("ScreenshotWindow", &filename)
    }

    #[zbus(name = "ScreenshotArea")]
    fn screenshot_area(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        _flash: bool,
        filename: String,
    ) -> (bool, String) {
        log::debug!(
            "ScreenshotArea({x}, {y}, {width}, {height}, _, {filename:?}) [→ full-screen]"
        );
        // xdg-desktop-portal Screenshot doesn't have an area variant; fall back
        // to full-screen (same as ScreenshotWindow).
        self.do_capture("ScreenshotArea", &filename)
    }
}

pub struct IdleMonitorService {
    backend: Arc<dyn IdleBackend>,
}

impl IdleMonitorService {
    pub fn new(backend: Arc<dyn IdleBackend>) -> Self {
        Self { backend }
    }
}

#[interface(name = "org.gnome.Mutter.IdleMonitor")]
impl IdleMonitorService {
    #[zbus(name = "GetIdletime")]
    fn get_idletime(&self) -> u64 {
        let ms = self.backend.idle_time_ms();
        log::debug!("GetIdletime → {ms} ms");
        ms
    }
}

/// Move the captured file to its target. Prefers `rename` (atomic, free on
/// the same fs) and falls back to `copy + remove` across filesystems.
fn place(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(src, dst)?;
            let _ = fs::remove_file(src);
            Ok(())
        }
    }
}
