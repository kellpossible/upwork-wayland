//! Screenshot backend trait + shared types.

use std::path::PathBuf;

use anyhow::Result;

pub mod portal;

/// A screenshot backend captures the desktop and returns the path to a PNG file.
///
/// The caller is responsible for moving/copying the file to its desired
/// destination. The file is in `$XDG_RUNTIME_DIR` (tmpfs) when produced by
/// xdg-desktop-portal.
pub trait ScreenshotBackend: Send + Sync {
    fn capture(&self) -> Result<PathBuf>;
}
