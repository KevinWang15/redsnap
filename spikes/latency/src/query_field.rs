//! The search query field (gpui has no built-in text field), adapted from
//! gpui's `input` example: IME composition, selection with mouse and keyboard,
//! clipboard, and scrolling long text.
//!
//! Emits [`QueryChanged`] when the committed text changes. Composing (marked)
//! IME text is drawn underlined but isn't a change, so typing pinyin doesn't
//! search on every keystroke.

use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, InteractiveElement, IntoElement,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement,
    Pixels, Point, Render, ShapedLine, SharedString, Style, Styled, TextRun, UTF16Selection, UnderlineStyle, Window,
    actions, div, fill, point, px, relative, rgba, size,
};

use crate::perf;

actions!(query_field, [Backspace, Delete, Left, Right, SelectLeft, SelectRight, SelectAll, Home, End, Paste, Cut, Copy]);

const CONTEXT: &str = "QueryField";

/// Registers the editing keys; call once at startup.
pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-c", Copy, c),
    ]);
}

/// The committed text changed.
pub struct QueryChanged;

pub struct QueryField {
    focus: FocusHandle,
    content: String,
    placeholder: SharedString,
    /// Byte offsets into `content`.
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    /// How far the text is scrolled left to keep the caret visible.
    scroll: Pixels,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl EventEmitter<QueryChanged> for QueryField {}

impl QueryField {
    pub fn new(placeholder: &str, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: String::new(),
            placeholder: placeholder.to_string().into(),
            selected: 0..0,
            reversed: false,
            marked: None,
            scroll: px(0.),
            last_layout: None,
            last_bounds: None,
            selecting: false,
        }
    }

    /// The committed text: without IME text still being composed.
    pub fn query(&self) -> String {
        match &self.marked {
            Some(m) => {
                let m = self.clamp_range(m);
                format!("{}{}", &self.content[..m.start], &self.content[m.end..])
            }
            None => self.content.clone(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        let changed = !self.content.is_empty();
        self.content.clear();
        self.selected = 0..0;
        self.reversed = false;
        self.marked = None;
        self.scroll = px(0.);
        cx.notify();
        if changed {
            cx.emit(QueryChanged);
        }
    }

    // Offsets also arrive from the OS (IME queries) and from the last layout,
    // which can be a frame behind the content. Every one is clamped to the
    // content and to a character boundary before use.

    fn clamp(&self, offset: usize) -> usize {
        clamp_offset(&self.content, offset)
    }

    fn clamp_range(&self, r: &Range<usize>) -> Range<usize> {
        clamp_range(&self.content, r)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.content[..offset].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.content[offset..].chars().next().map_or(offset, |c| offset + c.len_utf8())
    }

    fn cursor(&self) -> usize {
        if self.reversed { self.selected.start } else { self.selected.end }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp(offset);
        self.selected = offset..offset;
        self.reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.clamp(offset);
        self.selected = self.clamp_range(&self.selected);
        if self.reversed {
            self.selected.start = offset;
        } else {
            self.selected.end = offset;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify();
    }

    fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds, self.last_layout.as_ref()) else { return 0 };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        self.clamp(line.closest_index_for_x(position.x - bounds.left() + self.scroll))
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected.is_empty() { self.previous_boundary(self.cursor()) } else { self.selected.start };
        self.move_to(to, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let to = if self.selected.is_empty() { self.next_boundary(self.selected.end) } else { self.selected.end };
        self.move_to(to, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        perf::input_started();
        if self.selected.is_empty() {
            self.select_to(self.previous_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        perf::input_started();
        if self.selected.is_empty() {
            self.select_to(self.next_boundary(self.cursor()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let line: String = text.trim().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            self.replace_text_in_range(None, &line, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            let selected = self.clamp_range(&self.selected);
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[selected].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            self.copy(&Copy, window, cx);
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.selecting = true;
        let at = self.index_for_position(ev.position);
        if ev.modifiers.shift {
            self.select_to(at, cx);
        } else if ev.click_count >= 2 {
            self.select_all(&SelectAll, window, cx);
        } else {
            self.move_to(at, cx);
        }
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_for_position(ev.position), cx);
        }
    }

    fn utf16_range(&self, r: &Range<usize>) -> Range<usize> {
        range_to_utf16(&self.content, r)
    }

    fn byte_range(&self, r: &Range<usize>) -> Range<usize> {
        range_from_utf16(&self.content, r)
    }
}

impl EntityInputHandler for QueryField {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.byte_range(&range);
        adjusted.replace(self.utf16_range(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.utf16_range(&self.selected), reversed: self.reversed })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.utf16_range(r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            cx.emit(QueryChanged);
        }
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        perf::input_started();
        let range = range
            .as_ref()
            .map(|r| self.byte_range(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let range = self.clamp_range(&range);
        self.content.replace_range(range.clone(), text);
        let end = range.start + text.len();
        self.selected = end..end;
        self.reversed = false;
        self.marked = None;
        cx.notify();
        cx.emit(QueryChanged);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|r| self.byte_range(r))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());
        let range = self.clamp_range(&range);
        self.content.replace_range(range.clone(), text);
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = new_selected
            .map(|r| {
                // Relative to the inserted text, in its UTF-16 units.
                let base = byte_to_utf16(&self.content, range.start);
                utf16_to_byte(&self.content, base + r.start)..utf16_to_byte(&self.content, base + r.end)
            })
            .unwrap_or(range.start + text.len()..range.start + text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.byte_range(&range);
        let x = |i| bounds.left() + line.x_for_index(i) - self.scroll;
        Some(Bounds::from_corners(point(x(range.start), bounds.top()), point(x(range.end), bounds.bottom())))
    }

    fn character_index_for_point(&mut self, p: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let index = line.index_for_x(p.x - bounds.left() + self.scroll)?;
        Some(byte_to_utf16(&self.content, self.clamp(index)))
    }
}

/// `offset` within `text`, moved back to a character boundary.
fn clamp_offset(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// `r` with both ends clamped (see [`clamp_offset`]) and in order.
fn clamp_range(text: &str, r: &Range<usize>) -> Range<usize> {
    let (a, b) = (clamp_offset(text, r.start), clamp_offset(text, r.end));
    a.min(b)..a.max(b)
}

fn utf16_to_byte(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= offset {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn byte_to_utf16(text: &str, offset: usize) -> usize {
    text[..clamp_offset(text, offset)].chars().map(char::len_utf16).sum()
}

fn range_from_utf16(text: &str, r: &Range<usize>) -> Range<usize> {
    clamp_range(text, &(utf16_to_byte(text, r.start)..utf16_to_byte(text, r.end)))
}

fn range_to_utf16(text: &str, r: &Range<usize>) -> Range<usize> {
    let r = clamp_range(text, r);
    byte_to_utf16(text, r.start)..byte_to_utf16(text, r.end)
}

/// The text itself: shapes and paints the line, caret and selection.
struct TextElement {
    field: Entity<QueryField>,
}

struct Prepaint {
    line: ShapedLine,
    caret: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let field = self.field.read(cx);
        let style = window.text_style();
        let empty = field.content.is_empty();
        let (text, color) = if empty {
            (field.placeholder.to_string(), rgba(0xFFFF_FF59).into())
        } else {
            (field.content.clone(), style.color)
        };
        let (selected, cursor) =
            if empty { (0..0, 0) } else { (field.clamp_range(&field.selected), field.clamp(field.cursor())) };
        let marked = field.marked.as_ref().filter(|_| !empty).map(|m| field.clamp_range(m));
        let old_scroll = field.scroll;

        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match marked {
            Some(m) => {
                let underline = UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false };
                [
                    TextRun { len: m.start, ..run.clone() },
                    TextRun { len: m.end - m.start, underline: Some(underline), ..run.clone() },
                    TextRun { len: text.len() - m.end, ..run },
                ]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect()
            }
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(text.into(), font_size, &runs, None);

        // Keep the caret in view.
        let caret_x = line.x_for_index(cursor);
        let width = bounds.size.width - px(2.);
        let mut scroll = old_scroll.min((line.width - width).max(px(0.)));
        if caret_x - scroll > width {
            scroll = caret_x - width;
        } else if caret_x < scroll {
            scroll = caret_x;
        }
        self.field.update(cx, |f, _| f.scroll = scroll);

        let x = |i: usize| bounds.left() + line.x_for_index(i) - scroll;
        let (selection, caret) = if selected.is_empty() {
            let caret = Bounds::new(point(x(cursor), bounds.top()), size(px(1.5), bounds.size.height));
            (None, Some(fill(caret, rgba(0xFFFF_FFFF))))
        } else {
            let r = Bounds::from_corners(point(x(selected.start), bounds.top()), point(x(selected.end), bounds.bottom()));
            (Some(fill(r, rgba(0xE12D_4666))), None)
        };
        Prepaint { line, caret, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus, scroll) = {
            let f = self.field.read(cx);
            (f.focus.clone(), f.scroll)
        };
        window.handle_input(&focus, ElementInputHandler::new(bounds, self.field.clone()), cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            let origin = point(bounds.left() - scroll, bounds.top());
            prepaint.line.paint(origin, window.line_height(), window, cx).ok();
            if focus.is_focused(window)
                && let Some(caret) = prepaint.caret.take()
            {
                window.paint_quad(caret);
            }
        });
        let line = prepaint.line.clone();
        self.field.update(cx, |f, _| {
            f.last_layout = Some(line);
            f.last_bounds = Some(bounds);
        });
    }
}

impl Render for QueryField {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .flex_1()
            .flex()
            .items_center()
            .text_size(px(26.))
            .line_height(px(34.))
            .text_color(rgba(0xFFFF_FFFF))
            .child(TextElement { field: cx.entity() })
    }
}

impl Focusable for QueryField {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_from_stale_layouts_stay_inside_characters() {
        let text = "a密b";
        assert_eq!(clamp_offset(text, 99), 5);
        assert_eq!(clamp_offset(text, 2), 1);
        assert_eq!(clamp_offset(text, 4), 4);
        assert_eq!(clamp_range(text, &(9..2)), 1..5);
    }
}
