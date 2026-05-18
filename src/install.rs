use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::cli::InstallArgs;

const DESKTOP_TEMPLATE: &str = "\
[Desktop Entry]
Name=Upwork (Wayland)
Comment=Upwork desktop app with Wayland screenshot bridge
Exec={exe} run
Icon=upwork
Terminal=false
Type=Application
Categories=Network;Office;
StartupWMClass=upwork
";

pub fn run(args: InstallArgs) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current_exe")?;
    let exe = exe.canonicalize().unwrap_or(exe);

    let xdg_data_home = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").expect("HOME unset")).join(".local/share")
        });
    let dir = xdg_data_home.join("applications");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let path = dir.join("upwork-wayland.desktop");
    let new_contents = DESKTOP_TEMPLATE.replace("{exe}", &exe.display().to_string());

    if path.exists() {
        let old_contents =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;

        if old_contents == new_contents {
            log::info!("{} is already up to date", path.display());
            return Ok(());
        }

        if !args.force && !confirm_overwrite(&path, &old_contents, &new_contents)? {
            bail!("aborted");
        }
    }

    fs::write(&path, &new_contents).with_context(|| format!("writing {}", path.display()))?;
    log::info!("Wrote {}", path.display());
    Ok(())
}

fn confirm_overwrite(path: &std::path::Path, old: &str, new: &str) -> Result<bool> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    writeln!(out, "About to overwrite: {}", path.display())?;
    writeln!(out)?;
    writeln!(out, "─── existing ───────────────────────────────────────")?;
    write!(out, "{}", old)?;
    if !old.ends_with('\n') {
        writeln!(out)?;
    }
    writeln!(out, "─── new ────────────────────────────────────────────")?;
    write!(out, "{}", new)?;
    if !new.ends_with('\n') {
        writeln!(out)?;
    }
    writeln!(out, "────────────────────────────────────────────────────")?;
    write!(out, "Overwrite? [y/N] ")?;
    out.flush()?;

    let stdin = std::io::stdin();
    let mut answer = String::new();
    stdin.lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}
