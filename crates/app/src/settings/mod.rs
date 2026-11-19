//! The settings window: a gpui window in RedSnap's own process.
//!
//! The app loop owns the settings (`crate::app::State`). The window only
//! shows and edits them: every read and change is a request to the loop
//! (`config.get`, `config.save` with the revision it's based on, the
//! shortcut calls, …), answered there after validating it. Changes made
//! elsewhere (the tray, another save) come back as pushed events.

mod pages;
mod view;
mod widgets;

use anyhow::Result;
use futures::channel::oneshot;
use gpui::{App, AppContext, AsyncApp, Bounds, Point, TitlebarOptions, WindowBounds, WindowHandle, WindowKind, WindowOptions, px, size};
use serde_json::Value;

use crate::app::{Event, Events};
pub use view::{SettingsView, theme};

/// How the app loop answers a request.
pub type Reply = Box<dyn FnOnce(Result<Value, String>) + Send>;

/// Sends `method` to the app loop; the answer arrives on the receiver.
pub fn request(cx: &App, method: &str, params: Value) -> oneshot::Receiver<Result<Value, String>> {
    let (tx, rx) = oneshot::channel();
    let reply: Reply = Box::new(move |r| {
        let _ = tx.send(r);
    });
    let _ = cx.global::<Events>().0.unbounded_send(Event::Settings(method.to_string(), params, reply));
    rx
}

/// The settings window while it's open.
#[derive(Default)]
pub struct SettingsHost {
    window: Option<WindowHandle<SettingsView>>,
}

const SIZE: (f32, f32) = (980., 700.);

impl SettingsHost {
    /// Opens the settings window, or brings it forward if it's open.
    pub fn open(&mut self, cx: &mut AsyncApp) -> Result<()> {
        if let Some(w) = self.window
            && w.update(cx, |_, window, _| window.activate_window()).is_ok()
        {
            return Ok(());
        }
        let handle = cx.update(|cx| {
            let bounds = Bounds::centered(None, size(px(SIZE.0), px(SIZE.1)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // The page paints the title bar (view.rs); macOS keeps its
                    // traffic lights over it.
                    titlebar: Some(TitlebarOptions {
                        title: Some("RedSnap Settings".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(Point { x: px(14.), y: px(13.) }),
                    }),
                    focus: true,
                    show: true,
                    kind: WindowKind::Normal,
                    is_movable: true,
                    is_resizable: true,
                    is_minimizable: true,
                    app_id: Some("redsnap".into()),
                    window_min_size: Some(size(px(720.), px(480.))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| SettingsView::new(window, cx)),
            )
        })?;
        self.window = Some(handle);
        Ok(())
    }

    /// The window closed.
    pub fn closed(&mut self) {
        self.window = None;
    }

    /// Tells the open window about a change made elsewhere.
    pub fn push(&self, cx: &mut AsyncApp, event: &str, params: Value) {
        if let Some(w) = self.window {
            w.update(cx, |view, window, cx| view.pushed(event, params, window, cx)).ok();
        }
    }
}
