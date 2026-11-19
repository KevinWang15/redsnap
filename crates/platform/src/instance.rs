//! One RedSnap per user. The first process listens on a local endpoint (a
//! named pipe on Windows, a Unix socket in the data folder elsewhere); a
//! second one sends its command there (`redsnap toggle`) and exits.
//!
//! Protocol: newline-delimited JSON, one request and one reply per line.
//! - `{"method":"hello"}` → `{"pid":1234}` (the client lets that process take
//!   the foreground on Windows before asking it to show itself)
//! - `{"method":"cli","params":{"command":"toggle"}}` → `{"ok":true}`
//!
//! Lines over 64 KiB close the connection. The Electron settings app will use
//! the same endpoint with a token (M4).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::thread;

use anyhow::{Context as _, Result, bail};
use interprocess::local_socket::{ListenerOptions, Stream, prelude::*};
use serde::{Deserialize, Serialize};

const MAX_LINE: usize = 64 * 1024;

/// What a second launch asks the running RedSnap to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CliCommand {
    Show,
    Toggle,
    Settings,
    Quit,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "lowercase")]
enum Request {
    Hello,
    Cli { command: CliCommand },
}

pub enum Acquired {
    /// This process is RedSnap; commands from later launches arrive through
    /// the callback given to [`acquire`].
    Primary,
    /// Another RedSnap is running; talk to it with [`Client::send`].
    Secondary(Client),
}

pub struct Client {
    stream: BufReader<Stream>,
}

#[cfg(windows)]
fn name(data: &Path) -> Result<interprocess::local_socket::Name<'static>> {
    use interprocess::local_socket::GenericNamespaced;
    // Pipe names are global; the user and a hash of the data folder keep users
    // (and test or demo profiles) apart.
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let safe: String = user.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let hash = data.to_string_lossy().bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
    Ok(format!("redsnap-{safe}-{:08x}", hash as u32).to_ns_name::<GenericNamespaced>()?.into_owned())
}

#[cfg(unix)]
fn name(data: &Path) -> Result<interprocess::local_socket::Name<'static>> {
    use interprocess::local_socket::GenericFilePath;
    // The data folder is 0700, so only this user can reach the socket.
    let path = data.join("redsnap.sock");
    Ok(path.to_fs_name::<GenericFilePath>()?.into_owned())
}

/// Becomes the primary RedSnap, or finds the one already running.
pub fn acquire(data: &Path, on_command: impl Fn(CliCommand) + Send + 'static) -> Result<Acquired> {
    let endpoint = name(data)?;
    if let Ok(stream) = Stream::connect(endpoint.clone()) {
        return Ok(Acquired::Secondary(Client { stream: BufReader::new(stream) }));
    }
    // Nobody answered: a socket file left by a crash is replaced.
    let listener = ListenerOptions::new()
        .name(endpoint)
        .try_overwrite(true)
        .create_sync()
        .context("listening for other RedSnap launches")?;
    thread::Builder::new().name("instance".into()).spawn(move || {
        for conn in listener.incoming() {
            match conn {
                Ok(conn) => {
                    if let Err(e) = serve(conn, &on_command) {
                        log::debug!("instance connection: {e:#}");
                    }
                }
                Err(e) => log::warn!("instance listener: {e}"),
            }
        }
    })?;
    Ok(Acquired::Primary)
}

fn serve(conn: Stream, on_command: &impl Fn(CliCommand)) -> Result<()> {
    let mut reader = BufReader::new(conn);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.by_ref().take(MAX_LINE as u64 + 1).read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        if n > MAX_LINE {
            bail!("request too long");
        }
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Hello) => serde_json::json!({ "pid": std::process::id() }),
            Ok(Request::Cli { command }) => {
                on_command(command);
                serde_json::json!({ "ok": true })
            }
            Err(e) => serde_json::json!({ "error": e.to_string() }),
        };
        let mut out = reply.to_string();
        out.push('\n');
        reader.get_mut().write_all(out.as_bytes())?;
    }
}

impl Client {
    fn call(&mut self, request: &Request) -> Result<serde_json::Value> {
        let mut line = serde_json::to_string(request)?;
        line.push('\n');
        self.stream.get_mut().write_all(line.as_bytes())?;
        let mut reply = String::new();
        self.stream.read_line(&mut reply)?;
        Ok(serde_json::from_str(&reply)?)
    }

    /// Sends `command` to the running RedSnap. On Windows it first lets that
    /// process take the foreground; without that, it couldn't show itself on
    /// top when asked by another process.
    pub fn send(mut self, command: CliCommand) -> Result<()> {
        let hello = self.call(&Request::Hello)?;
        #[cfg(windows)]
        if let Some(pid) = hello.get("pid").and_then(|p| p.as_u64()) {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid as u32) };
        }
        let _ = hello;
        let reply = self.call(&Request::Cli { command })?;
        if let Some(e) = reply.get("error") {
            bail!("RedSnap refused: {e}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn second_launch_reaches_the_first() {
        let dir = std::env::temp_dir().join(format!("redsnap-instance-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (tx, rx) = mpsc::channel();
        let first = acquire(&dir, move |c| tx.send(c).unwrap()).unwrap();
        assert!(matches!(first, Acquired::Primary));
        let Acquired::Secondary(client) = acquire(&dir, |_| {}).unwrap() else { panic!("expected secondary") };
        client.send(CliCommand::Toggle).unwrap();
        assert_eq!(rx.recv().unwrap(), CliCommand::Toggle);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
