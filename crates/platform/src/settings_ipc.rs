//! The channel between RedSnap and its settings app (PLAN.md §4.5).
//!
//! Each settings session gets its own endpoint with a random name (a named
//! pipe on Windows, a socket in the 0700 data folder elsewhere) and a random
//! token. Both reach Electron through its environment, never its command
//! line. The first line Electron sends must be `hello` with the token;
//! anything else closes the connection.
//!
//! Framing: one JSON object per line, at most 1 MiB.
//! - request  `{"id": 7, "method": "config.save", "params": {...}}`
//! - response `{"id": 7, "result": {...}}` or `{"id": 7, "error": "..."}`
//! - event    `{"event": "config.changed", "params": {...}}` (RedSnap → Electron)

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use anyhow::{Context as _, Result, bail};
use interprocess::local_socket::{ListenerOptions, Stream, prelude::*};
use serde_json::{Value, json};

const MAX_LINE: usize = 1 << 20;

/// One connection, read on the IPC thread and written from any thread
/// (`&Stream` reads and writes).
#[derive(Clone)]
struct Shared(Arc<Stream>);

impl Read for Shared {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&*self.0).read(buf)
    }
}

impl Write for Shared {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (&*self.0).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (&*self.0).flush()
    }
}

/// Replies to one request, from any thread, once.
pub type Reply = Box<dyn FnOnce(Result<Value, String>) + Send>;

/// Called on the IPC thread for each request after `hello`.
pub type OnRequest = Box<dyn Fn(String, Value, Reply) + Send + Sync>;

/// A settings session: where Electron should connect, with what token.
pub struct Session {
    /// For `REDSNAP_IPC_PATH`: a pipe path on Windows, a socket path elsewhere.
    pub path: String,
    pub token: String,
    writer: Arc<Mutex<Option<Shared>>>,
}

fn random_hex(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn write_line(writer: &Mutex<Option<Shared>>, value: &Value) -> bool {
    let mut guard = writer.lock().unwrap();
    let Some(w) = guard.as_mut() else { return false };
    let mut line = value.to_string();
    line.push('\n');
    if w.write_all(line.as_bytes()).and_then(|_| w.flush()).is_err() {
        *guard = None;
        return false;
    }
    true
}

impl Session {
    /// Opens the endpoint and waits (on a thread) for the settings app.
    /// `on_closed` runs when its connection ends (window closed, crash).
    pub fn start(data: &Path, on_request: OnRequest, on_closed: Box<dyn FnOnce() + Send>) -> Result<Session> {
        let token = random_hex(16)?;
        let id = random_hex(8)?;
        #[cfg(windows)]
        let (name, path) = {
            use interprocess::local_socket::GenericNamespaced;
            let n = format!("redsnap-settings-{id}");
            (n.clone().to_ns_name::<GenericNamespaced>()?.into_owned(), format!(r"\\.\pipe\{n}"))
        };
        #[cfg(unix)]
        let (name, path) = {
            use interprocess::local_socket::GenericFilePath;
            let p = data.join(format!("settings-{id}.sock"));
            (p.clone().to_fs_name::<GenericFilePath>()?.into_owned(), p.to_string_lossy().to_string())
        };
        #[cfg(windows)]
        let _ = data;
        let listener = ListenerOptions::new().name(name).create_sync().context("opening the settings channel")?;
        let writer: Arc<Mutex<Option<Shared>>> = Arc::new(Mutex::new(None));
        let shared = writer.clone();
        let expected = token.clone();
        let cleanup = path.clone();
        thread::Builder::new().name("settings-ipc".into()).spawn(move || {
            // The first connection that proves it has the token is the session.
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let conn = Shared(Arc::new(conn));
                let send = conn.clone();
                let mut reader = BufReader::new(conn);
                let mut line = String::new();
                let hello = read_line(&mut reader, &mut line).ok().and_then(|_| serde_json::from_str::<Value>(&line).ok());
                let ok = hello.as_ref().is_some_and(|h| {
                    h["method"] == "hello" && h["params"]["token"].as_str() == Some(expected.as_str())
                });
                if !ok {
                    log::warn!("settings channel: a connection without the token was refused");
                    continue;
                }
                *shared.lock().unwrap() = Some(send);
                let id = hello.as_ref().map(|h| h["id"].clone()).unwrap_or(Value::Null);
                write_line(&shared, &json!({ "id": id, "result": { "ok": true } }));
                serve(&mut reader, &shared, &on_request);
                break;
            }
            *shared.lock().unwrap() = None;
            #[cfg(unix)]
            let _ = std::fs::remove_file(&cleanup);
            #[cfg(windows)]
            let _ = cleanup;
            on_closed();
        })?;
        Ok(Session { path, token, writer })
    }

    /// Sends an event to the settings app, if it's connected.
    pub fn push(&self, event: &str, params: Value) -> bool {
        write_line(&self.writer, &json!({ "event": event, "params": params }))
    }
}

fn read_line(reader: &mut impl BufRead, line: &mut String) -> Result<usize> {
    line.clear();
    let n = reader.by_ref().take(MAX_LINE as u64 + 1).read_line(line)?;
    if n > MAX_LINE {
        bail!("message over 1 MiB");
    }
    Ok(n)
}

fn serve(reader: &mut impl BufRead, writer: &Arc<Mutex<Option<Shared>>>, on_request: &OnRequest) {
    let mut line = String::new();
    loop {
        match read_line(reader, &mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            log::warn!("settings channel: a line that isn't JSON");
            return;
        };
        let (Some(method), id) = (req["method"].as_str().map(str::to_string), req["id"].clone()) else { continue };
        let params = req.get("params").cloned().unwrap_or(Value::Null);
        let w = writer.clone();
        on_request(
            method,
            params,
            Box::new(move |result| {
                let msg = match result {
                    Ok(v) => json!({ "id": id, "result": v }),
                    Err(e) => json!({ "id": id, "error": e }),
                };
                write_line(&w, &msg);
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interprocess::local_socket::Stream;

    fn connect(path: &str) -> Stream {
        #[cfg(unix)]
        {
            use interprocess::local_socket::GenericFilePath;
            Stream::connect(path.to_fs_name::<GenericFilePath>().unwrap()).unwrap()
        }
        #[cfg(windows)]
        {
            use interprocess::local_socket::GenericNamespaced;
            let n = path.trim_start_matches(r"\\.\pipe\");
            Stream::connect(n.to_ns_name::<GenericNamespaced>().unwrap()).unwrap()
        }
    }

    #[test]
    fn token_then_requests_and_events() {
        let dir = std::env::temp_dir().join(format!("redsnap-ipc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        let session = Session::start(
            &dir,
            Box::new(|method, params, reply| reply(Ok(json!({ "echo": method, "params": params })))),
            Box::new(move || closed_tx.send(()).unwrap()),
        )
        .unwrap();
        // Without the token: refused.
        let mut bad = BufReader::new(connect(&session.path));
        bad.get_mut().write_all(b"{\"id\":1,\"method\":\"hello\",\"params\":{\"token\":\"nope\"}}\n").unwrap();
        let mut line = String::new();
        assert_eq!(bad.read_line(&mut line).unwrap_or(0), 0);
        // With it: requests answered by id, events pushed.
        let mut good = BufReader::new(connect(&session.path));
        let hello = format!("{{\"id\":1,\"method\":\"hello\",\"params\":{{\"token\":\"{}\"}}}}\n", session.token);
        good.get_mut().write_all(hello.as_bytes()).unwrap();
        line.clear();
        good.read_line(&mut line).unwrap();
        assert!(line.contains("\"ok\":true"), "{line}");
        good.get_mut().write_all(b"{\"id\":2,\"method\":\"config.get\"}\n").unwrap();
        line.clear();
        good.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], 2);
        assert_eq!(v["result"]["echo"], "config.get");
        assert!(session.push("config.changed", json!({ "revision": 3 })));
        line.clear();
        good.read_line(&mut line).unwrap();
        assert!(line.contains("config.changed"));
        drop(good);
        closed_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
