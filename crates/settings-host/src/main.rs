//! RedSnap Settings in the system webview (WebView2 on Windows, WKWebView on
//! macOS, WebKitGTK on Linux): the React + Tailwind UI in settings-ui/,
//! without shipping a browser.
//!
//! RedSnap starts this with its private channel in the environment
//! (`REDSNAP_IPC_PATH`, `REDSNAP_IPC_TOKEN`).
//! The page gets `window.redsnap` (`call`, `onEvent`, `platform`) from an
//! initialization script; calls go through `window.ipc.postMessage` to this
//! process, which checks them against an allow-list and forwards them to
//! RedSnap. The UI is served from the `web/` folder next to this executable
//! (`REDSNAP_SETTINGS_WEB` overrides it in development) through a private
//! URL scheme, never `file://` or the network.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use serde_json::{Value, json};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use wry::http::{Response, header::CONTENT_TYPE};
use wry::{PageLoadEvent, WebViewBuilder};

/// The requests the page may make; anything else is refused here.
const ALLOWED: &[&str] = &[
    "config.get",
    "config.save",
    "app.info",
    "hotkey.suspend",
    "hotkey.resume",
    "hotkey.test",
    "apps.stats",
    "apps.reload",
    "usage.reset",
    "clipboard.stats",
    "clipboard.clear",
    "logs.open",
    "data.open",
];

const SCHEME: &str = "redsnap";

/// What the page is served from: `http://redsnap.localhost` on Windows
/// (WebView2 maps custom schemes that way), `redsnap://localhost` elsewhere.
fn start_url() -> &'static str {
    if cfg!(windows) { "http://redsnap.localhost/index.html" } else { "redsnap://localhost/index.html" }
}

/// Events for the window's event loop, from the RedSnap channel thread.
enum Ui {
    /// A message from RedSnap (a response or an event), for the page.
    FromRedSnap(Value),
    /// The page has loaded: show the window.
    Loaded,
    /// The channel closed: RedSnap is gone.
    Closed,
}

fn platform() -> &'static str {
    // Node's `process.platform` names, which the UI uses.
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    }
}

/// Defines `window.redsnap` before the page's own scripts run.
fn bridge_script() -> String {
    format!(
        r#"(() => {{
  const pending = new Map();
  const listeners = new Set();
  let nextId = 1;
  window.__redsnapReceive = (msg) => {{
    if (msg.id != null && pending.has(msg.id)) {{
      const p = pending.get(msg.id);
      pending.delete(msg.id);
      if ('error' in msg) p.reject(new Error(msg.error)); else p.resolve(msg.result);
    }} else if (msg.event) {{
      for (const cb of listeners) cb(msg.event, msg.params);
    }}
  }};
  window.redsnap = Object.freeze({{
    call: (method, params) => new Promise((resolve, reject) => {{
      const id = nextId++;
      pending.set(id, {{ resolve, reject }});
      window.ipc.postMessage(JSON.stringify({{ id, method, params: params ?? {{}} }}));
    }}),
    onEvent: (cb) => {{ listeners.add(cb); return () => listeners.delete(cb); }},
    platform: {platform:?},
  }});
}})();"#,
        platform = platform()
    )
}

/// The RedSnap channel: write requests from the UI thread, read on its own.
struct Channel {
    writer: Mutex<Box<dyn Write + Send>>,
    next_id: Mutex<u64>,
    /// Our request id → the page's id, for replies.
    page_ids: Mutex<std::collections::HashMap<u64, Value>>,
}

impl Channel {
    fn send(&self, method: &str, params: Value, page_id: Option<Value>) -> Result<()> {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        if let Some(p) = page_id {
            self.page_ids.lock().unwrap().insert(id, p);
        }
        let mut line = json!({ "id": id, "method": method, "params": params }).to_string();
        line.push('\n');
        let mut w = self.writer.lock().unwrap();
        w.write_all(line.as_bytes())?;
        w.flush()?;
        Ok(())
    }
}

/// A shared stream half that reads and writes (`&Stream` does both).
struct Shared(Arc<interprocess::local_socket::Stream>);

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

/// Connects, says hello with the token, and starts the reader thread, which
/// hands every later message to the UI.
fn connect(proxy: EventLoopProxy<Ui>) -> Result<Arc<Channel>> {
    let path = std::env::var("REDSNAP_IPC_PATH").context("start Settings from RedSnap (no REDSNAP_IPC_PATH)")?;
    let token = std::env::var("REDSNAP_IPC_TOKEN").context("start Settings from RedSnap (no REDSNAP_IPC_TOKEN)")?;
    // Nothing this process starts needs it.
    unsafe { std::env::remove_var("REDSNAP_IPC_TOKEN") };
    let stream = Arc::new(redsnap_platform::settings_ipc::connect(&path).context("connecting to RedSnap")?);
    let mut reader = BufReader::new(Shared(stream.clone()));
    let channel = Arc::new(Channel {
        writer: Mutex::new(Box::new(Shared(stream))),
        next_id: Mutex::new(0),
        page_ids: Mutex::new(Default::default()),
    });
    channel.send("hello", json!({ "token": token }), None)?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply: Value = serde_json::from_str(&line).context("RedSnap's reply to hello")?;
    if reply.get("error").is_some() || reply["result"]["ok"] != true {
        bail!("RedSnap refused the connection");
    }
    let ch = channel.clone();
    std::thread::Builder::new().name("redsnap-channel".into()).spawn(move || {
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Ok(mut msg) = serde_json::from_str::<Value>(&line) else { continue };
            // Replies carry our id; the page knows its own.
            if let Some(id) = msg["id"].as_u64() {
                match ch.page_ids.lock().unwrap().remove(&id) {
                    Some(page_id) => msg["id"] = page_id,
                    None => continue, // a reply to our own request (settings.shown)
                }
            }
            if proxy.send_event(Ui::FromRedSnap(msg)).is_err() {
                break;
            }
        }
        let _ = proxy.send_event(Ui::Closed);
    })?;
    Ok(channel)
}

fn web_root() -> PathBuf {
    if let Some(p) = std::env::var_os("REDSNAP_SETTINGS_WEB") {
        return p.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().map(|d| d.join("web")).unwrap_or_else(|| "web".into())
}

/// A file of the built UI, by URL path. Only files inside `root`.
fn serve(root: &Path, path: &str) -> Response<Vec<u8>> {
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    let not_found = || Response::builder().status(404).body(Vec::new()).unwrap();
    if rel.split('/').any(|part| part == ".." || part.is_empty()) {
        return not_found();
    }
    let Ok(bytes) = std::fs::read(root.join(rel)) else { return not_found() };
    let mime = match rel.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    };
    Response::builder().header(CONTENT_TYPE, mime).body(bytes).unwrap()
}

fn main() {
    if let Err(e) = run() {
        // No console in release builds on Windows: say it in a log and a dialog.
        eprintln!("RedSnap Settings: {e:#}");
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
            MessageBoxW(std::ptr::null_mut(), wide(&format!("{e:#}")).as_ptr(), wide("RedSnap Settings").as_ptr(), MB_ICONERROR);
        }
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let event_loop = EventLoopBuilder::<Ui>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let channel = connect(proxy.clone())?;

    let window = WindowBuilder::new()
        .with_title("RedSnap Settings")
        .with_inner_size(tao::dpi::LogicalSize::new(980.0, 700.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(780.0, 540.0))
        .with_window_icon(tao::window::Icon::from_rgba(redsnap_platform::icon::app_icon_rgba(64), 64, 64).ok())
        // Shown once the page has loaded, so it never flashes empty.
        .with_visible(false)
        .build(&event_loop)?;

    let root = web_root();
    if !root.join("index.html").exists() {
        bail!("the settings UI is missing ({} has no index.html)", root.display());
    }
    let ipc_channel = channel.clone();
    let page_proxy = proxy.clone();
    // The webview's own data (cache, local storage) goes under RedSnap's data
    // folder, never next to the executable (which may be read-only).
    let data = std::env::var_os("REDSNAP_SETTINGS_DATA").map(PathBuf::from);
    let mut context = wry::WebContext::new(data);
    let builder = WebViewBuilder::new_with_web_context(&mut context)
        .with_background_color((13, 16, 23, 255))
        .with_initialization_script(bridge_script())
        .with_custom_protocol(SCHEME.into(), move |_id, request| serve(&root, request.uri().path()).map(Into::into))
        .with_url(start_url())
        // The window shows bundled files only.
        .with_navigation_handler(|url| url.starts_with("http://redsnap.localhost") || url.starts_with("redsnap://"))
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_on_page_load_handler(move |event, _url| {
            if let PageLoadEvent::Finished = event {
                let _ = page_proxy.send_event(Ui::Loaded);
            }
        })
        .with_ipc_handler(move |request| {
            let Ok(msg) = serde_json::from_str::<Value>(request.body()) else { return };
            let (id, method) = (msg["id"].clone(), msg["method"].as_str().unwrap_or("").to_string());
            if !ALLOWED.contains(&method.as_str()) {
                let _ = proxy.send_event(Ui::FromRedSnap(json!({ "id": id, "error": format!("not allowed: {method}") })));
                return;
            }
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            if let Err(e) = ipc_channel.send(&method, params, Some(id.clone())) {
                let _ = proxy.send_event(Ui::FromRedSnap(json!({ "id": id, "error": format!("RedSnap is gone: {e}") })));
            }
        });

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    let webview = builder.build(&window)?;
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        builder.build_gtk(window.default_vbox().context("no GTK box for the webview")?)?
    };

    let mut shown = false;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                // Hide first: tearing the webview down uncovers the window's
                // own white background for a moment.
                window.set_visible(false);
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(Ui::Loaded) if !shown => {
                shown = true;
                window.set_visible(true);
                window.set_focus();
                // For RedSnap's log: requested → shown.
                let _ = channel.send("settings.shown", json!({}), None);
            }
            Event::UserEvent(Ui::Loaded) => {}
            Event::UserEvent(Ui::FromRedSnap(msg)) => {
                if msg["event"] == "focus" {
                    window.set_visible(true);
                    window.set_focus();
                    return;
                }
                let _ = webview.evaluate_script(&format!("window.__redsnapReceive({msg})"));
            }
            Event::UserEvent(Ui::Closed) => {
                window.set_visible(false);
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
