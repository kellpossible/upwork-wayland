//! Round-trip integration test: spawn `upwork-wayland serve`, make a
//! `Screenshot` D-Bus call via `gdbus`, verify a PNG comes out.
//!
//! Requires a real Wayland session with xdg-desktop-portal installed and
//! (on KDE Plasma 6.5+) the "Allow" permission already granted for
//! `upwork-wayland`. Marked `#[ignore]` so it stays out of normal
//! `cargo test` runs; invoke with `cargo test -- --ignored`.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[test]
#[ignore = "requires Wayland session + xdg-desktop-portal + portal consent already granted"]
fn screenshot_round_trip_produces_png() {
    let bin = env!("CARGO_BIN_EXE_upwork-wayland");
    let target = std::env::temp_dir().join("upwork-wayland-test.png");
    let _ = std::fs::remove_file(&target);

    let mut bridge = Command::new(bin)
        .arg("serve")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn upwork-wayland serve");

    // Give zbus time to claim names.
    thread::sleep(Duration::from_millis(800));

    let out = Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.gnome.Shell.Screenshot",
            "--object-path",
            "/org/gnome/Shell/Screenshot",
            "--method",
            "org.gnome.Shell.Screenshot.Screenshot",
            "true",
            "false",
            target.to_str().unwrap(),
        ])
        .output()
        .expect("run gdbus");

    let _ = bridge.kill();
    let _ = bridge.wait();

    assert!(
        out.status.success(),
        "gdbus call failed: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with("(true,"),
        "gdbus returned non-success: {stdout}"
    );

    let mut header = [0u8; 8];
    std::fs::File::open(&target)
        .expect("open captured png")
        .read_exact(&mut header)
        .expect("read png header");
    assert_eq!(&header, b"\x89PNG\r\n\x1a\n", "captured file is not a PNG");
}
