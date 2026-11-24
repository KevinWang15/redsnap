//! Start at login: an HKCU `Run` value on Windows, a LaunchAgent on macOS,
//! an XDG autostart entry on Linux. Each points at the current executable;
//! [`refresh`] re-points an enabled entry when the app has moved.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

#[cfg(any(target_os = "windows", target_os = "linux"))]
const NAME: &str = "RedSnap";

pub fn is_enabled() -> bool {
    imp::read().is_some()
}

pub fn set(enabled: bool) -> Result<()> {
    let exe = std::env::current_exe().context("finding RedSnap's executable")?;
    if enabled {
        imp::write(&exe)
    } else {
        imp::remove()
    }
}

/// If start at login is on but points somewhere else, point it here.
pub fn refresh() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
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
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut len,
            )
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
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        anyhow::ensure!(r == ERROR_SUCCESS, "writing the Run value failed ({r})");
        Ok(())
    }

    pub fn remove() -> Result<()> {
        let (key, name) = (wide(RUN), wide(NAME));
        let r = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        anyhow::ensure!(
            r == ERROR_SUCCESS || r == 2,
            "removing the Run value failed ({r})"
        );
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    const LABEL: &str = "com.kevinwang15.redsnap";

    fn plist() -> Option<PathBuf> {
        Some(
            dirs::home_dir()?
                .join("Library/LaunchAgents")
                .join(format!("{LABEL}.plist")),
        )
    }

    pub(super) fn read_from(path: &Path) -> Option<PathBuf> {
        let value = plist::Value::from_file(path).ok()?;
        let args = value.as_dictionary()?.get("ProgramArguments")?.as_array()?;
        let exe = args.first()?.as_string()?;
        if exe.is_empty() {
            return None;
        }
        Some(PathBuf::from(exe))
    }

    pub(super) fn write_to(path: &Path, exe: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut dict = plist::Dictionary::new();
        dict.insert("Label".into(), plist::Value::String(LABEL.into()));
        dict.insert(
            "ProgramArguments".into(),
            plist::Value::Array(vec![
                plist::Value::String(exe.to_string_lossy().into_owned()),
                plist::Value::String("--background".into()),
            ]),
        );
        dict.insert("RunAtLoad".into(), plist::Value::Boolean(true));
        plist::Value::Dictionary(dict)
            .to_file_xml(path)
            .with_context(|| format!("writing LaunchAgent {}", path.display()))?;
        Ok(())
    }

    pub(super) fn remove_at(path: &Path) -> Result<()> {
        match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn read() -> Option<PathBuf> {
        read_from(&plist()?)
    }

    pub fn write(exe: &Path) -> Result<()> {
        let path = plist().context("no home directory")?;
        write_to(&path, exe)
    }

    pub fn remove() -> Result<()> {
        match plist() {
            Some(p) => remove_at(&p),
            None => Ok(()),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::imp;
    use std::path::PathBuf;

    #[test]
    fn launch_agent_plist_round_trips_special_chars() {
        let dir = tempfile::tempdir().unwrap();
        let plist_path = dir.path().join("com.kevinwang15.redsnap.plist");
        let exe = PathBuf::from("/Applications/Red & Snap <Test>/Contents/MacOS/redsnap");

        assert_eq!(imp::read_from(&plist_path), None);
        imp::write_to(&plist_path, &exe).unwrap();
        assert_eq!(imp::read_from(&plist_path), Some(exe));

        // Verify ProgramArguments includes --background and RunAtLoad is true.
        let value = plist::Value::from_file(&plist_path).unwrap();
        let dict = value.as_dictionary().unwrap();
        assert_eq!(
            dict.get("RunAtLoad").and_then(|v| v.as_boolean()),
            Some(true)
        );
        let args = dict.get("ProgramArguments").unwrap().as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[1].as_string(), Some("--background"));

        imp::remove_at(&plist_path).unwrap();
        assert_eq!(imp::read_from(&plist_path), None);
        // Removing an already-absent file succeeds.
        imp::remove_at(&plist_path).unwrap();
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
        let path = exec
            .strip_prefix('"')
            .and_then(|s| s.split('"').next())
            .unwrap_or_else(|| exec.split(' ').next().unwrap_or(""));
        Some(PathBuf::from(path))
    }

    pub fn write(exe: &Path) -> Result<()> {
        let path = entry().context("no config directory")?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        // Desktop-entry Exec quoting: backslash, quote, dollar and backtick are escaped.
        let quoted: String = exe
            .display()
            .to_string()
            .chars()
            .flat_map(|c| match c {
                '\\' | '"' | '$' | '`' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
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
