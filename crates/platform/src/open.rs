//! Running results: launching apps, opening URLs and paths, showing a file in
//! its folder, system commands. Everything starts detached processes or OS
//! calls with typed arguments; no shell command line is built from text.

use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use redsnap_core::item::{LaunchTarget, SystemCommand, url_allowed};

/// Starts a program without waiting for it and without a console window.
fn spawn(program: &str, args: &[&std::ffi::OsStr]) -> Result<()> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own session, so it outlives RedSnap and gets no signals from it.
        unsafe {
            cmd.pre_exec(|| {
                libc_setsid();
                Ok(())
            });
        }
    }
    let mut child = cmd.spawn().with_context(|| format!("starting {program}"))?;
    // Reap it in the background so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(unix)]
fn libc_setsid() {
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

#[cfg(windows)]
fn shell_execute(file: &str, params: Option<&str>) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, file) = (wide("open"), wide(file));
    let params = params.map(wide);
    let r = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success as a value above 32.
    if r as usize <= 32 {
        bail!("the shell couldn't open it (code {})", r as usize);
    }
    Ok(())
}

pub fn open_url(url: &str) -> Result<()> {
    if !url_allowed(url) {
        bail!("not an http(s) or settings URL: {url}");
    }
    #[cfg(windows)]
    return shell_execute(url, None);
    #[cfg(target_os = "macos")]
    return spawn("open", &[url.as_ref()]);
    #[cfg(target_os = "linux")]
    return spawn("xdg-open", &[url.as_ref()]);
}

pub fn open_path(path: &Path) -> Result<()> {
    if !path.exists() {
        bail!("{} doesn't exist", path.display());
    }
    #[cfg(windows)]
    return shell_execute(&path.to_string_lossy(), None);
    #[cfg(target_os = "macos")]
    return spawn("open", &[path.as_os_str()]);
    #[cfg(target_os = "linux")]
    return spawn("xdg-open", &[path.as_os_str()]);
}

/// Shows `path` selected in Explorer / Finder, or opens its folder on Linux.
pub fn reveal(path: &Path) -> Result<()> {
    #[cfg(windows)]
    return spawn(
        "explorer.exe",
        &[format!("/select,{}", path.display()).as_ref()],
    );
    #[cfg(target_os = "macos")]
    return spawn("open", &["-R".as_ref(), path.as_os_str()]);
    #[cfg(target_os = "linux")]
    return spawn("xdg-open", &[path.parent().unwrap_or(path).as_os_str()]);
}

pub fn launch(target: &LaunchTarget) -> Result<()> {
    match target {
        #[cfg(windows)]
        LaunchTarget::AppsFolder(id) => shell_execute(&format!("shell:AppsFolder\\{id}"), None),
        #[cfg(not(windows))]
        LaunchTarget::AppsFolder(_) => bail!("AppsFolder ids exist only on Windows"),
        LaunchTarget::Path(p) => open_path(p),
        #[cfg(target_os = "linux")]
        LaunchTarget::Desktop { id, .. } => {
            // gio and gtk-launch both honor the desktop file's Exec, Path and
            // Terminal keys.
            let id = id.trim_end_matches(".desktop");
            spawn("gtk-launch", &[id.as_ref()])
                .or_else(|_| spawn("gio", &["launch".as_ref(), id.as_ref()]))
        }
        #[cfg(not(target_os = "linux"))]
        LaunchTarget::Desktop { .. } => bail!("desktop files exist only on Linux"),
    }
}

/// The system commands this OS supports.
pub fn system_commands() -> &'static [SystemCommand] {
    use SystemCommand::*;
    &[Lock, Sleep, LogOut, Restart, ShutDown, EmptyTrash]
}

pub fn run_system(cmd: SystemCommand) -> Result<()> {
    imp::run(cmd)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows_sys::Win32::System::Power::SetSuspendState;
    use windows_sys::Win32::System::Shutdown::{
        EWX_LOGOFF, ExitWindowsEx, LockWorkStation, SHTDN_REASON_FLAG_PLANNED,
    };
    use windows_sys::Win32::UI::Shell::{
        SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHEmptyRecycleBinW,
    };

    pub fn run(cmd: SystemCommand) -> Result<()> {
        let ok = unsafe {
            match cmd {
                SystemCommand::Lock => LockWorkStation() != 0,
                SystemCommand::Sleep => SetSuspendState(false, false, false),
                SystemCommand::LogOut => ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_FLAG_PLANNED) != 0,
                // shutdown.exe holds the privilege dance (SE_SHUTDOWN_NAME).
                SystemCommand::Restart => {
                    return spawn(
                        "shutdown.exe",
                        &["/r".as_ref(), "/t".as_ref(), "0".as_ref()],
                    );
                }
                SystemCommand::ShutDown => {
                    return spawn(
                        "shutdown.exe",
                        &["/s".as_ref(), "/t".as_ref(), "0".as_ref()],
                    );
                }
                SystemCommand::EmptyTrash => {
                    let flags = SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND;
                    let hr = SHEmptyRecycleBinW(std::ptr::null_mut(), std::ptr::null(), flags);
                    // S_OK, or "already empty".
                    hr >= 0 || hr == 0x8000FFFFu32 as i32
                }
            }
        };
        anyhow::ensure!(ok, "{cmd:?} failed");
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn osascript(script: &str) -> Result<()> {
        spawn("osascript", &["-e".as_ref(), script.as_ref()])
    }

    pub fn run(cmd: SystemCommand) -> Result<()> {
        match cmd {
            // Sleeping the display locks the session when "require password
            // immediately" is on (the default).
            SystemCommand::Lock => spawn("pmset", &["displaysleepnow".as_ref()]),
            SystemCommand::Sleep => spawn("pmset", &["sleepnow".as_ref()]),
            SystemCommand::LogOut => osascript("tell application \"System Events\" to log out"),
            SystemCommand::Restart => osascript("tell application \"System Events\" to restart"),
            SystemCommand::ShutDown => osascript("tell application \"System Events\" to shut down"),
            SystemCommand::EmptyTrash => osascript("tell application \"Finder\" to empty trash"),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub fn run(cmd: SystemCommand) -> Result<()> {
        let args: &[&str] = match cmd {
            SystemCommand::Lock => &["loginctl", "lock-session"],
            SystemCommand::Sleep => &["systemctl", "suspend"],
            SystemCommand::LogOut => &["loginctl", "terminate-session", ""],
            SystemCommand::Restart => &["systemctl", "reboot"],
            SystemCommand::ShutDown => &["systemctl", "poweroff"],
            SystemCommand::EmptyTrash => &["gio", "trash", "--empty"],
        };
        let session = std::env::var("XDG_SESSION_ID").unwrap_or_default();
        let rest: Vec<&std::ffi::OsStr> = args[1..]
            .iter()
            .map(|a| {
                if a.is_empty() {
                    session.as_ref()
                } else {
                    a.as_ref()
                }
            })
            .collect();
        spawn(args[0], &rest)
    }
}
