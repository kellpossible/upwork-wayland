//! `LD_PRELOAD` shim injected into the Upwork process (see `src/launcher.rs`).
//!
//! # Why this exists
//!
//! Upwork's `uta_native.node` captures the screen one of two ways, chosen by a
//! native Wayland probe (`define_session_type`, which `dlopen`s
//! libwayland-client and calls `wl_display_connect`):
//!
//!   * **Wayland detected** → capture via a D-Bus screenshot interface
//!     (`org.gnome.Shell.Screenshot` / portal). BUT a *separate* JS-level guard
//!     (`if (utaNative.isWayland()) return <empty>`) fires first and blocks
//!     every *periodic* capture, so only the first (ungated) screenshot of a
//!     session ever lands.
//!   * **Wayland not detected** → capture in-process via
//!     `gdk_pixbuf_get_from_window` on the X11 root window. Under XWayland that
//!     window is unredirected, so the image is all black.
//!
//! Both consumers read the *same* detection byte, so no environment variable
//! can satisfy both (one wants it true, the other false). The launcher forces
//! `isWayland = false` (via `WAYLAND_DISPLAY=""`) so the JS guard stops blocking
//! and Upwork takes the `gdk_pixbuf_get_from_window` path — and this shim
//! interposes that call, returning a *real* frame captured through our bridge
//! (`org.gnome.Shell.Screenshot` → xdg-desktop-portal) instead of the black
//! root window.
//!
//! Compiled standalone as a cdylib by `build.rs` and embedded into the main
//! binary; it has no crate dependencies (just `std` + two `extern "C"` decls).

use std::ffi::{CString, c_char, c_int, c_void};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

unsafe extern "C" {
    /// libc/libdl — resolved against the host process at load time.
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// `RTLD_NEXT`: resolve the *next* definition after this shim, i.e. the real
/// `gdk_pixbuf_new_from_file` in the already-loaded GdkPixbuf.
const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

/// `GdkPixbuf *gdk_pixbuf_new_from_file(const char *filename, GError **error)`
type NewFromFileFn = unsafe extern "C" fn(*const c_char, *mut *mut c_void) -> *mut c_void;

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Interposes `GdkPixbuf *gdk_pixbuf_get_from_window(GdkWindow*, x, y, w, h)`.
///
/// We ignore the window/geometry: the portal hands back the full screen, which
/// matches the root-window geometry Upwork asks for. On any failure we return
/// NULL, exactly as the real function may — Upwork already handles that.
#[unsafe(no_mangle)]
pub extern "C" fn gdk_pixbuf_get_from_window(
    _window: *mut c_void,
    _src_x: c_int,
    _src_y: c_int,
    _width: c_int,
    _height: c_int,
) -> *mut c_void {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut path = std::env::temp_dir();
    path.push(format!("upwork-wayland-cap-{}-{}.png", std::process::id(), seq));
    let path = path.to_string_lossy().into_owned();

    // Ask our bridge to capture (it routes through xdg-desktop-portal). The
    // bridge writes the PNG to the path we hand it and returns (success, path).
    let captured = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell.Screenshot",
            "--object-path",
            "/org/gnome/Shell/Screenshot",
            "--method",
            "org.gnome.Shell.Screenshot.Screenshot",
            "false",
            "false",
            &path,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !captured {
        return std::ptr::null_mut();
    }

    let pixbuf = match CString::new(path.clone()) {
        Ok(c) => unsafe {
            let sym = dlsym(RTLD_NEXT, c"gdk_pixbuf_new_from_file".as_ptr());
            if sym.is_null() {
                std::ptr::null_mut()
            } else {
                let real: NewFromFileFn = std::mem::transmute(sym);
                real(c.as_ptr(), std::ptr::null_mut())
            }
        },
        Err(_) => std::ptr::null_mut(),
    };

    let _ = std::fs::remove_file(&path);
    pixbuf
}
