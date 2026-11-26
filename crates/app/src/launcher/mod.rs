//! The search window: one pre-created window for the life of the process,
//! shown and hidden with OS calls (PLAN.md §5.1), drawn to docs/design.md.

pub mod clip;
pub mod query_field;
mod view;

use std::sync::Arc;
use std::time::Instant;

use gpui::components::SelectableTextEvent;
use gpui::{
    AppContext, AsyncApp, Context, Entity, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
    Window, WindowHandle, actions,
};
use redsnap_core::files::{FileRequest, FileResults, Files};
use redsnap_core::item::{Action, Item};
use redsnap_core::search::{Engine, Hit};
use redsnap_core::usage::UsageStore;
use redsnap_platform::files::FileSearch;
use redsnap_platform::window::{self as os, Native};

use crate::execute;
use crate::perf;
use query_field::{QueryChanged, QueryField};

actions!(
    launcher,
    [
        SelectNext,
        SelectPrev,
        PageDown,
        PageUp,
        PreviewDown,
        PreviewUp,
        CopyEntry,
        TogglePin,
        DeleteEntry,
        Run,
        RunSecondary,
        ToggleActions,
        Dismiss,
        Run1,
        Run2,
        Run3,
        Run4,
        Run5,
        Run6,
        Run7,
        Run8,
        Run9
    ]
);

const CONTEXT: &str = "Launcher";
pub const WIDTH: f32 = 760.;
/// Results kept beyond the visible rows, for scrolling.
const LIMIT: usize = 200;

pub fn height(rows: usize) -> f32 {
    16. + 56. + 12. + rows as f32 * 66. + 44. + 16.
}

pub fn bind_keys(cx: &mut gpui::App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("ctrl-n", SelectNext, c),
        KeyBinding::new("up", SelectPrev, c),
        KeyBinding::new("ctrl-p", SelectPrev, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("shift-pagedown", PreviewDown, c),
        KeyBinding::new("shift-pageup", PreviewUp, c),
        KeyBinding::new(secondary!("c"), CopyEntry, c),
        KeyBinding::new(secondary!("p"), TogglePin, c),
        // Cmd+Backspace in Finder; Ctrl+Delete elsewhere (Ctrl+Backspace means
        // "delete the previous word" in a text box).
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-backspace"
            } else {
                "ctrl-delete"
            },
            DeleteEntry,
            c,
        ),
        KeyBinding::new("enter", Run, c),
        KeyBinding::new(secondary!("enter"), RunSecondary, c),
        KeyBinding::new("tab", ToggleActions, c),
        KeyBinding::new(secondary!("k"), ToggleActions, c),
        KeyBinding::new("escape", Dismiss, c),
        KeyBinding::new(secondary!("1"), Run1, c),
        KeyBinding::new(secondary!("2"), Run2, c),
        KeyBinding::new(secondary!("3"), Run3, c),
        KeyBinding::new(secondary!("4"), Run4, c),
        KeyBinding::new(secondary!("5"), Run5, c),
        KeyBinding::new(secondary!("6"), Run6, c),
        KeyBinding::new(secondary!("7"), Run7, c),
        KeyBinding::new(secondary!("8"), Run8, c),
        KeyBinding::new(secondary!("9"), Run9, c),
    ]);
}

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub rows: usize,
    /// The panel's corner radius (the window's, when the OS rounds it).
    pub radius: f32,
    /// Opacity of the panel's fill; below 1 the OS backdrop shows through.
    pub fill: f32,
}

/// The action panel (Tab): what can be done with the selected result.
struct ActionPanel {
    item: Arc<Item>,
    actions: Vec<Action>,
    selected: usize,
}

pub struct Launcher {
    field: Entity<QueryField>,
    engine: Engine,
    /// What the query wants from the file index or a folder, shared with
    /// the engine's files provider.
    files: Arc<Files>,
    file_search: Option<FileSearch>,
    /// The last request sent, so each is sent once.
    files_asked: Option<FileRequest>,
    usage: UsageStore,
    query: String,
    hits: Vec<Hit>,
    selected: usize,
    top: usize,
    panel: Option<ActionPanel>,
    /// A session-ending command waiting for a second Enter.
    confirming: Option<Arc<Item>>,
    /// A one-line message above the results (a settings problem, a failed
    /// action). Cleared by typing.
    notice: Option<String>,
    pub native: Option<Native>,
    pub visible: bool,
    /// Opened with the clipboard shortcut: the clipboard view until hidden.
    clip_sticky: bool,
    clip: clip::ClipView,
    /// The window in front before this show, to paste into.
    paste_target: Option<redsnap_platform::clipboard::Target>,
    /// Whether the window has been active since it was last shown.
    had_focus: bool,
    opts: Options,
    _subs: Vec<gpui::Subscription>,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Launcher {
    pub fn new(
        (engine, files): (Engine, Arc<Files>),
        file_search: Option<FileSearch>,
        usage: UsageStore,
        opts: Options,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| QueryField::new("Search apps and commands", cx));
        let clip = clip::ClipView::new(cx);
        let field_focus = field.focus_handle(cx);
        let subs = vec![
            cx.subscribe(&field, |this, _, _: &QueryChanged, cx| {
                this.query_changed(cx)
            }),
            cx.subscribe(
                &clip.preview,
                |this, _, ev: &SelectableTextEvent, cx| match ev {
                    SelectableTextEvent::Focus => {
                        this.clip.panel = None;
                        this.field.update(cx, |f, cx| f.clear_selection(cx));
                        cx.notify();
                    }
                    SelectableTextEvent::SelectionChanged(_) => cx.notify(),
                    SelectableTextEvent::Copy(_) => this.clip_flash_copied(cx),
                    SelectableTextEvent::Blur => {}
                },
            ),
            cx.on_focus(&field_focus, window, |this, _, cx| {
                this.clip_clear_preview_focus_state(cx);
            }),
            // Hide when focus moves elsewhere, but only after the window has
            // had focus since it was shown: right after mapping, X11 (and a
            // slow foreground switch on Windows) reports it inactive first.
            cx.observe_window_activation(window, |this, window, cx| {
                log::debug!("window active: {}", window.is_window_active());
                if window.is_window_active() {
                    // X11 can report a hidden window active; that doesn't count.
                    this.had_focus |= this.visible;
                } else if this.visible && this.had_focus {
                    this.hide(cx);
                }
            }),
        ];
        let mut this = Launcher {
            field,
            engine,
            files,
            file_search,
            files_asked: None,
            usage,
            query: String::new(),
            hits: Vec::new(),
            selected: 0,
            top: 0,
            panel: None,
            confirming: None,
            notice: None,
            native: None,
            visible: false,
            clip_sticky: false,
            clip,
            paste_target: None,
            had_focus: false,
            opts,
            _subs: subs,
        };
        this.refresh();
        this
    }

    /// Settings changed what search finds (providers, keywords).
    pub fn set_engine(&mut self, (engine, files): (Engine, Arc<Files>), cx: &mut Context<Self>) {
        self.engine = engine;
        self.files = files;
        self.files_asked = None;
        self.refresh();
        cx.notify();
    }

    /// A rescan replaced the app catalog: search again, keeping the selection.
    pub fn apps_changed(&mut self, cx: &mut Context<Self>) {
        self.refresh();
        cx.notify();
    }

    pub fn set_notice(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        self.notice = notice;
        cx.notify();
    }

    /// Runs the search for the current query. Keeps the selection on the same
    /// item when it is still there.
    fn refresh(&mut self) {
        let keep = self.hits.get(self.selected).map(|h| h.item.id.clone());
        let usage = self.usage.snapshot();
        let (q, hits) = self.engine.search(&self.query, &usage, now_ms(), LIMIT);
        self.hits = hits;
        // Files come from another thread: ask (once per request); the answer
        // searches again (`files_arrived`).
        let want = self.files.request(&q);
        if want.is_some()
            && want != self.files_asked
            && let (Some(s), Some(r)) = (&self.file_search, &want)
        {
            s.request(r.clone());
        }
        self.files_asked = want;
        self.selected = keep
            .and_then(|id| self.hits.iter().position(|h| h.item.id == id))
            .unwrap_or(0);
        self.scroll_to_selected();
        perf::results_ready();
    }

    /// The file index or a folder listing answered.
    pub fn files_arrived(&mut self, r: FileResults, cx: &mut Context<Self>) {
        self.files.set_results(r);
        if self.visible && !self.in_clipboard() {
            self.refresh();
            cx.notify();
            self.paint_soon(cx);
        }
    }

    /// Whether the clipboard view is showing: opened with its shortcut, or
    /// the query starts with `cb `.
    pub(super) fn in_clipboard(&self) -> bool {
        self.clip_sticky || self.query.trim_start().starts_with("cb ")
    }

    /// New clipboard entries arrived.
    pub fn clipboard_changed(&mut self, cx: &mut Context<Self>) {
        if self.visible && self.in_clipboard() {
            self.refresh_clipboard(cx);
            cx.notify();
        }
    }

    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let was_clipboard = self.in_clipboard();
        self.query = self.field.read(cx).query();
        log::debug!("query {:?}", self.query);
        if self.in_clipboard() && !was_clipboard {
            // Entering the clipboard view: a fresh list and preview.
            self.clip.reset(cx);
        }
        self.panel = None;
        self.confirming = None;
        self.notice = None;
        if self.in_clipboard() {
            self.clip.selected = 0;
            self.clip.top = 0;
            self.refresh_clipboard(cx);
            perf::results_ready();
            cx.notify();
            self.paint_soon(cx);
            return;
        }
        // A new query starts at the top.
        self.hits.clear();
        self.refresh();
        cx.notify();
        self.paint_soon(cx);
    }

    /// Paint as soon as this update returns instead of on gpui's next vsync
    /// tick (PLAN.md §7: measured 0.8 ms vs 6.3 ms per keystroke).
    fn paint_soon(&self, cx: &mut Context<Self>) {
        if os::CAN_PAINT_NOW
            && let Some(n) = self.native
        {
            cx.spawn(async move |_, _| os::paint_now(n)).detach();
        }
    }

    fn scroll_to_selected(&mut self) {
        let rows = self.opts.rows;
        if self.selected < self.top {
            self.top = self.selected;
        } else if self.selected >= self.top + rows {
            self.top = self.selected + 1 - rows;
        }
    }

    /// Fresh state for a new show: empty query, the most used items.
    fn reset(&mut self, clipboard: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.clip_sticky = clipboard;
        self.clip.reset(cx);
        if let Some(e) = execute::take_error() {
            self.notice = Some(e);
        }
        self.had_focus = false;
        // Folders may have changed since the last show: list them again.
        self.files_asked = None;
        self.field.update(cx, |f, cx| f.clear(cx));
        self.query.clear();
        self.panel = None;
        self.confirming = None;
        self.hits.clear();
        self.refresh();
        if clipboard {
            self.refresh_clipboard(cx);
        }
        window.focus(&self.field.focus_handle(cx), cx);
        cx.notify();
    }

    pub fn hide(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        log::debug!("hide");
        self.visible = false;
        self.had_focus = false;
        self.panel = None;
        self.confirming = None;
        self.clip_clear_preview_focus_state(cx);
        let native = self.native;
        cx.spawn(async move |_, _| {
            if let Some(n) = native {
                os::hide(n);
            }
        })
        .detach();
    }

    /// Hides the window and clears the query (session lock / sleep).
    pub fn hide_and_clear(&mut self, cx: &mut Context<Self>) {
        self.field.update(cx, |f, cx| f.clear(cx));
        self.query.clear();
        self.hide(cx);
    }

    fn move_selection(&mut self, to: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.in_clipboard() && self.clip_panel_move(to, cx) {
            return;
        }
        if self.panel.is_none() && self.in_clipboard() {
            if self.clip_preview_focused(window, cx) {
                window.focus(&self.field.focus_handle(cx), cx);
            }
            self.clip_move(to, cx);
            self.paint_soon(cx);
            return;
        }
        if let Some(panel) = &mut self.panel {
            let n = panel.actions.len() as isize;
            panel.selected = to.clamp(0, n - 1) as usize;
        } else if !self.hits.is_empty() {
            perf::input_started();
            self.selected = to.clamp(0, self.hits.len() as isize - 1) as usize;
            self.confirming = None;
            self.scroll_to_selected();
            perf::results_ready();
        }
        cx.notify();
        self.paint_soon(cx);
    }

    fn current(&self) -> isize {
        if self.in_clipboard() {
            return self.clip_current() as isize;
        }
        self.panel.as_ref().map_or(self.selected, |p| p.selected) as isize
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            cx.propagate();
            return;
        }
        self.move_selection(self.current() + 1, window, cx);
    }

    fn select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            cx.propagate();
            return;
        }
        self.move_selection(self.current() - 1, window, cx);
    }

    fn page_down(&mut self, _: &PageDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        self.move_selection(self.current() + self.opts.rows as isize, window, cx);
    }

    fn page_up(&mut self, _: &PageUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        self.move_selection(self.current() - self.opts.rows as isize, window, cx);
    }

    /// Ctrl/Cmd+C with nothing selected in the field or preview: copy the
    /// selected clipboard entry (no paste).
    fn copy_entry(&mut self, _: &CopyEntry, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if self.in_clipboard() {
            self.clip_use(false, cx);
        }
    }

    fn preview_down(&mut self, _: &PreviewDown, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if self.in_clipboard() {
            self.clip_page(true, cx);
        }
    }

    fn preview_up(&mut self, _: &PreviewUp, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if self.in_clipboard() {
            self.clip_page(false, cx);
        }
    }

    fn toggle_pin(&mut self, _: &TogglePin, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if self.in_clipboard() {
            self.clip_pin(cx);
        }
    }

    fn delete_entry(&mut self, _: &DeleteEntry, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if self.in_clipboard() {
            self.clip_delete(cx);
        }
    }

    fn toggle_actions(&mut self, _: &ToggleActions, window: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            cx.propagate();
            return;
        }
        if self.in_clipboard() {
            if self.clip_preview_focused(window, cx) {
                window.focus(&self.field.focus_handle(cx), cx);
            }
            self.clip_toggle_panel(cx);
            return;
        }
        if self.panel.take().is_none()
            && let Some(hit) = self
                .hits
                .get(self.selected)
                .filter(|h| h.item.primary != Action::Nothing)
        {
            // Built only now, for the one selected item.
            let item = hit.item.clone();
            let mut actions = vec![item.primary.clone()];
            actions.extend(item.secondary.clone());
            let path = view::panel_path(&item).map(|p| Action::CopyText(p.display().to_string()));
            for copy in [Some(Action::CopyText(item.title.clone())), path]
                .into_iter()
                .flatten()
            {
                if !actions.contains(&copy) {
                    actions.push(copy);
                }
            }
            self.panel = Some(ActionPanel {
                item,
                actions,
                selected: 0,
            });
        }
        cx.notify();
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            cx.propagate();
            return;
        }
        if self.panel.take().is_some()
            || self.confirming.take().is_some()
            || self.clip.panel.take().is_some()
        {
            cx.notify();
        } else {
            self.hide(cx);
        }
    }

    fn run(&mut self, _: &Run, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            cx.propagate();
            return;
        }
        if self.in_clipboard() {
            self.clip_enter(cx);
            return;
        }
        if let Some(panel) = self.panel.take() {
            let action = panel.actions[panel.selected].clone();
            self.execute(panel.item, action, cx);
        } else if let Some(hit) = self.hits.get(self.selected) {
            let item = hit.item.clone();
            self.execute(item.clone(), item.primary.clone(), cx);
        }
    }

    fn run_secondary(&mut self, _: &RunSecondary, _: &mut Window, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if let Some(hit) = self.hits.get(self.selected)
            && let Some(action) = hit.item.secondary.clone()
        {
            let item = hit.item.clone();
            self.execute(item, action, cx);
        }
    }

    pub(super) fn run_row(&mut self, slot: usize, cx: &mut Context<Self>) {
        if self.clip_context_menu_open(cx) {
            return;
        }
        if let Some(hit) = self.hits.get(self.top + slot) {
            let item = hit.item.clone();
            self.execute(item.clone(), item.primary.clone(), cx);
        }
    }

    /// Typing while the clipboard preview pane has focus routes back to the
    /// search field so filtering stays instant without clicking the input.
    pub(super) fn on_key_down(
        &mut self,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.clip_preview_focused(window, cx) || self.clip_context_menu_open(cx) {
            return;
        }
        let mods = ev.keystroke.modifiers;
        if mods.control || mods.alt || mods.platform || mods.function {
            return;
        }
        if ev.keystroke.key == "backspace" {
            window.focus(&self.field.focus_handle(cx), cx);
            self.field.update(cx, |f, cx| f.delete_backward(cx));
            cx.stop_propagation();
        } else if let Some(text) = ev.keystroke.key_char.as_deref()
            && !text.is_empty()
            && !text.chars().all(char::is_control)
        {
            window.focus(&self.field.focus_handle(cx), cx);
            self.field.update(cx, |f, cx| f.insert_text(text, cx));
            cx.stop_propagation();
        }
    }

    /// Runs `action` for `item`: asks for confirmation first for commands that
    /// end the session, teaches ranking, hides the window, then acts.
    fn execute(&mut self, item: Arc<Item>, action: Action, cx: &mut Context<Self>) {
        match &action {
            Action::Nothing => return,
            // Into a folder: the query becomes its path, the window stays.
            Action::Browse(text) => {
                let text = text.clone();
                self.panel = None;
                self.field.update(cx, |f, cx| f.set_text(&text, cx));
                return;
            }
            _ => {}
        }
        if let Action::System(c) = &action
            && c.needs_confirmation()
            && self.confirming.as_ref().is_none_or(|i| i.id != item.id)
        {
            self.confirming = Some(item);
            cx.notify();
            return;
        }
        self.confirming = None;
        if item.learnable {
            self.usage.record_pick(&self.query, &item.id, now_ms());
        }
        self.hide(cx);
        if let Action::Snippet(id) = action {
            // Typed into the app that was in front, once RedSnap has hidden.
            let target = self.paste_target;
            let _ = cx
                .global::<crate::app::Events>()
                .0
                .unbounded_send(crate::app::Event::InsertSnippet(id, target));
            return;
        }
        execute::run(action, cx);
    }
}

impl Focusable for Launcher {
    fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.field.focus_handle(cx)
    }
}

/// Shows the window, finished: reset to the default list, place it on the
/// monitor under the cursor, paint while cloaked, reveal, focus.
/// Runs as a task so no OS call happens inside a gpui update.
pub async fn show(
    handle: WindowHandle<Launcher>,
    started: Instant,
    clipboard: bool,
    cx: &mut AsyncApp,
) {
    perf::show_started(started);
    // Before RedSnap takes the foreground: where Enter in the clipboard view pastes.
    let target = redsnap_platform::clipboard::foreground();
    let prepared = handle.update(cx, |launcher, window, cx| {
        launcher.paste_target = target;
        launcher.reset(clipboard, window, cx);
        launcher.visible = true;
        (launcher.native, launcher.opts)
    });
    let Ok((native, opts)) = prepared else { return };
    let Some(n) = native else { return };
    log::debug!("show");
    let target_size = (WIDTH, height(opts.rows));
    os::set_cloaked(n, true);
    os::place_at_cursor(n, target_size);
    os::show(n);
    os::ensure_placed_at_cursor(n, target_size);
    os::paint_now(n);
    os::set_cloaked(n, false);
    // The process that got the hotkey may take the foreground directly; gpui's
    // own activate replays creation bounds and injects a fake Alt keypress on
    // Windows, and spawns a second deferred makeKeyAndOrderFront on macOS.
    os::take_focus(n);
    if cfg!(target_os = "linux") {
        handle
            .update(cx, |_, window, _| window.activate_window())
            .ok();
    }
}
