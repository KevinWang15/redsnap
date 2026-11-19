//! Start at login: an HKCU `Run` value on Windows, a LaunchAgent on macOS,
//! an XDG autostart entry on Linux. Each points at the current executable;
//! [`refresh`] re-points an enabled entry when the app has moved.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

const NAME: &str = "RedSnap";

pub fn is_enabled() -> bool {
    imp::read().is_some()
}

pub fn set(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe().context("finding RedSnap's executable")?;
    if enabled { imp::write(&exe) } else { imp::remove() }
}

/// If start at login is on but points somewhere else, point it here.
pub fn refresh() {
    let Ok(exe) = std::env::current_exe() else { return };
    if let Some(current) = imp::read()
        && current != exe
        && let Err(e) = imp::write(&exe)
    {
        log::warn!("re-pointing start at login: {e:#}");
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn read() -> Option<PathBuf> {
        let (key, name) = (wide(RUN), wide(NAME));
        let mut buf = vec![0u16; 2048];
        let mut len = (buf.len() * 2) as u32;
        let r = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
        };
        if r != ERROR_SUCCESS {
            return None;
        }
        let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
        // Stored as `"C:\path\redsnap.exe" --background`.
        let path = s.trim().strip_prefix('"')?.split('"').next()?;
        Some(PathBuf::from(path))
    }

    pub fn write(exe: &Path) -> Result<()> {
        let value = wide(&format!("\"{}\" --background", exe.display()));
        let (key, name) = (wide(RUN), wide(NAME));
        let r = unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32)
        };
        anyhow::ensure!(r == ERROR_SUCCESS, "writing the Run value failed ({r})");
        Ok(())
    }

    pub fn remove() -> Result<()> {
        let (key, name) = (wide(RUN), wide(NAME));
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        anyhow::ensure!(r == ERROR_SUCCESS || r == 2, "removing the Run value failed ({r})");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    const LABEL: &str = "com.kevinwang15.redsnap";

    fn plist() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join("Library/LaunchAgents").join(format!("{LABEL}.plist")))
    }

    pub fn read() -> Option<PathBuf> {
        let text = std::fs::read_to_string(plist()?).ok()?;
        let start = text.find("<string>/")? + "<string>".len();
        let end = start + text[start..].find("</string>")?;
        Some(PathBuf::from(&text[start..end]))
    }

    pub fn write(exe: &Path) -> Result<()> {
        let path = plist().context("no home directory")?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let exe = exe.display().to_string().replace('&', "&amp;").replace('<', "&lt;");
        std::fs::write(
            &path,
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--background</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#
            ),
        )?;
        Ok(())
    }

    pub fn remove() -> Result<()> {
        match plist().map(std::fs::remove_file) {
            Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn entry() -> Option<PathBuf> {
        Some(dirs::config_dir()?.join("autostart/redsnap.desktop"))
    }

    pub fn read() -> Option<PathBuf> {
        let text = std::fs::read_to_string(entry()?).ok()?;
        let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?;
        let path = exec.strip_prefix('"').and_then(|s| s.split('"').next()).unwrap_or_else(|| exec.split(' ').next().unwrap_or(""));
        Some(PathBuf::from(path))
    }

    pub fn write(exe: &Path) -> Result<()> {
        let path = entry().context("no config directory")?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        // Desktop-entry Exec quoting: backslash, quote, dollar and backtick are escaped.
        let quoted: String = exe.display().to_string().chars().flat_map(|c| match c {
            '\\' | '"' | '$' | '`' => vec!['\\', c],
            c => vec![c],
        }).collect();
        std::fs::write(
            &path,
            format!(
                "[Desktop Entry]\nType=Application\nName={NAME}\nExec=\"{quoted}\" --background\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n"
            ),
        )?;
        Ok(())
    }

    pub fn remove() -> Result<()> {
        match entry().map(std::fs::remove_file) {
            Some(Err(e)) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}
