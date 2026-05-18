//! xdg-desktop-portal Screenshot backend.
//!
//! Calls `org.freedesktop.portal.Screenshot.Screenshot` on the session bus and
//! waits for the matching `org.freedesktop.portal.Request.Response` signal.
//! On success, returns the path that the portal wrote the PNG to (typically
//! under `$XDG_RUNTIME_DIR`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::ScreenshotBackend;

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";

/// The app_id we announce to the portal. Must match a `.desktop` file the
/// portal can find — see `src/install.rs`, which writes
/// `~/.local/share/applications/upwork-wayland.desktop`.
///
/// Without this, the Plasma 6.5+ permission-grant dialog falls back to the
/// generic "An app wants to take screenshots".
const APP_ID: &str = "upwork-wayland";

pub struct PortalBackend {
    conn: Connection,
}

impl PortalBackend {
    pub fn new() -> Result<Self> {
        let conn = Connection::session().context("connecting to session bus for portal")?;
        register_app_id(&conn, APP_ID);
        Ok(Self { conn })
    }
}

/// Declare our identity to xdg-desktop-portal. Best-effort: older portal
/// versions (< 1.18) don't expose the Registry interface; we just log and
/// continue, in which case the permission dialog stays generic.
fn register_app_id(conn: &Connection, app_id: &str) {
    let registry = match Proxy::new(
        conn,
        PORTAL_BUS,
        PORTAL_PATH,
        "org.freedesktop.host.portal.Registry",
    ) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("portal Registry proxy unavailable: {e}");
            return;
        }
    };
    let options: HashMap<&str, Value> = HashMap::new();
    match registry.call::<_, _, ()>("Register", &(app_id, options)) {
        Ok(()) => log::info!("registered with portal as app_id={app_id:?}"),
        Err(e) => log::warn!(
            "portal Registry.Register({app_id:?}) failed: {e} \
             (consent dialog will not show this app's name/icon)"
        ),
    }
}

impl ScreenshotBackend for PortalBackend {
    fn capture(&self) -> Result<PathBuf> {
        // Unique-per-call handle_token. The portal uses this together with our
        // unique-name to derive the Request object's path; we predict that
        // path so we can subscribe to its Response signal BEFORE making the
        // call (avoiding a race against a very fast portal).
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let token = format!("upwork_wayland_{}_{}", std::process::id(), nanos);

        let unique = self
            .conn
            .unique_name()
            .ok_or_else(|| anyhow!("no unique D-Bus name on session connection"))?;
        let sender_escaped = unique
            .as_str()
            .strip_prefix(':')
            .unwrap_or(unique.as_str())
            .replace('.', "_");
        let request_path_str =
            format!("/org/freedesktop/portal/desktop/request/{sender_escaped}/{token}");
        let request_path: OwnedObjectPath = ObjectPath::try_from(request_path_str.clone())
            .with_context(|| format!("invalid request path {request_path_str}"))?
            .into();

        let request_proxy = Proxy::new(
            &self.conn,
            PORTAL_BUS,
            request_path.as_ref(),
            "org.freedesktop.portal.Request",
        )
        .context("creating Request proxy")?;
        let mut signals = request_proxy
            .receive_signal("Response")
            .context("subscribing to Response signal")?;

        let mut options: HashMap<&str, Value> = HashMap::new();
        options.insert("handle_token", Value::from(token.as_str()));
        options.insert("interactive", Value::from(false));

        let screenshot_proxy = Proxy::new(
            &self.conn,
            PORTAL_BUS,
            PORTAL_PATH,
            "org.freedesktop.portal.Screenshot",
        )
        .context("creating Screenshot proxy")?;

        let returned_handle: OwnedObjectPath = screenshot_proxy
            .call("Screenshot", &("", options))
            .context("calling org.freedesktop.portal.Screenshot.Screenshot")?;
        if returned_handle.as_str() != request_path.as_str() {
            log::warn!(
                "portal returned handle {:?} but we predicted {:?} — Response signal may be missed",
                returned_handle.as_str(),
                request_path.as_str()
            );
        }

        let msg = signals
            .next()
            .ok_or_else(|| anyhow!("portal Response signal stream closed"))?;
        let (response, results): (u32, HashMap<String, OwnedValue>) = msg
            .body()
            .deserialize()
            .context("deserializing portal Response signal body")?;

        match response {
            0 => {}
            1 => return Err(anyhow!("user cancelled the portal screenshot")),
            2 => return Err(anyhow!("portal screenshot ended with an error")),
            other => return Err(anyhow!("portal Response code {other}")),
        }

        let uri_val = results
            .get("uri")
            .ok_or_else(|| anyhow!("portal Response had no 'uri' field"))?;
        let uri: &str = uri_val
            .downcast_ref()
            .map_err(|_| anyhow!("portal 'uri' was not a string"))?;

        uri_to_path(uri)
    }
}

fn uri_to_path(uri: &str) -> Result<PathBuf> {
    let path = uri
        .strip_prefix("file://")
        .ok_or_else(|| anyhow!("portal returned non-file URI: {uri}"))?;
    let decoded = percent_decode(path)?;
    Ok(PathBuf::from(decoded))
}

fn percent_decode(s: &str) -> Result<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = hex_digit(bytes[i + 1])?;
            let lo = hex_digit(bytes[i + 2])?;
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).context("decoded URI is not valid UTF-8")
}

fn hex_digit(b: u8) -> Result<u8> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(anyhow!("invalid hex digit '{}' in percent encoding", b as char)),
    }
}
