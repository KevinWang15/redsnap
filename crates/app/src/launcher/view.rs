//! Drawing the search window (docs/design.md).

use gpui::{
    AnyElement, BoxShadow, Context, Hsla, InteractiveElement, IntoElement, StatefulInteractiveElement, ObjectFit, ParentElement, Render,
    SharedString, Styled, StyledImage, Window, div, img, linear_color_stop, linear_gradient, point,
    prelude::FluentBuilder, px, rgb, rgba,
};
use redsnap_core::item::{Action, IconRef, Item};

use super::*;

fn white(alpha: f32) -> Hsla {
    let mut c: Hsla = rgb(0xFFFFFF).into();
    c.a = alpha;
    c
}

const RED: u32 = 0xE8344E;
const RADIUS: f32 = 20.;

fn with_alpha(rgb: u32, alpha: f32) -> gpui::Rgba {
    rgba((rgb << 8) | (alpha.clamp(0., 1.) * 255.).round() as u32)
}

fn shortcut(n: usize) -> String {
    if cfg!(target_os = "macos") { format!("⌘{n}") } else { format!("Ctrl {n}") }
}

impl Render for Launcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.in_clipboard() && self.preview_unmeasured() {
            // Draw once more when the preview knows its size (for the scrollbar).
            window.request_animation_frame();
        }
        let has_query = !self.field.read(cx).is_empty();
        let rows = self.opts.rows;
        let count = self.hits.len();
        let list: Vec<AnyElement> =
            (self.top..(self.top + rows).min(count)).map(|i| self.row(i, i == self.selected, i - self.top, cx)).collect();
        // With a fill under the glass of `alpha`; opaque when there's no
        // picture of the screen (capture failed or isn't available).
        let alpha = if self.backdrop.is_some() { self.opts.alpha } else { 0.97 };
        let radius = px(RADIUS);
        let layer = || div().absolute().top_0().left_0().size_full().rounded(radius);

        let search_row = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .h(px(56.))
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(12.))
                    .bg(rgba(0x0B0F16E6))
                    .border_1()
                    .border_color(rgba(0xE12D4673))
                    .child(self.field.clone())
                    .when(has_query, |d| {
                        d.child(
                            div()
                                .id("clear")
                                .size(px(26.))
                                .rounded_full()
                                .bg(white(0.10))
                                .hover(|s| s.bg(white(0.18)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(15.))
                                .text_color(white(0.75))
                                .child("×")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.field.update(cx, |f, cx| f.clear(cx));
                                    window.focus(&this.field.focus_handle(cx));
                                })),
                        )
                    }),
            )
            .child(logo_button());

        let in_clip = self.in_clipboard();
        let results = if in_clip { self.clip_body(cx) } else { div()
            .relative()
            .mt(px(12.))
            .h(px(rows as f32 * 66.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .when_some(self.notice.clone(), |d, notice| {
                d.child(
                    div()
                        .px(px(14.))
                        .py(px(8.))
                        .rounded(px(10.))
                        .bg(rgba(0xE8344E26))
                        .border_1()
                        .border_color(rgba(0xE8344E66))
                        .text_size(px(13.))
                        .text_color(white(0.9))
                        .child(notice),
                )
            })
            .children(list)
            .when_some(self.panel.as_ref(), |d, panel| d.child(action_panel(panel)))
            .into_any_element() };

        let footer_status: SharedString = match &self.confirming {
            Some(item) => format!("Press Enter again to {}", item.title.to_lowercase()).into(),
            None if in_clip => {
                let n = self.clip.found.len();
                format!("Clipboard · {n} entr{}", if n == 1 { "y" } else { "ies" }).into()
            }
            None => format!("{count} result{}", if count == 1 { "" } else { "s" }).into(),
        };
        let copy_key: &'static str = if cfg!(target_os = "macos") { "⌘C" } else { "Ctrl C" };
        let footer = div()
            .mt_auto()
            .h(px(44.))
            .border_t_1()
            .border_color(white(0.07))
            .flex()
            .items_center()
            .text_size(px(13.))
            .child(
                div()
                    .text_color(if self.confirming.is_some() { rgb(0xFF8090).into() } else { white(0.5) })
                    .child(footer_status),
            )
            .child(div().flex_1())
            .child(key_hint("↑↓", "Navigate"))
            .child(div().w(px(16.)))
            .child(key_hint("↵", if in_clip { "Paste" } else { "Open" }))
            .child(div().w(px(16.)))
            .when(in_clip, |d| d.child(key_hint(copy_key, "Copy")).child(div().w(px(16.))))
            .child(key_hint("⇥", "Actions"));

        let content = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .p(px(16.))
            .text_color(white(1.0))
            .child(search_row)
            .child(results)
            .child(footer);

        div()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::run_secondary))
            .on_action(cx.listener(Self::toggle_actions))
            .on_action(cx.listener(Self::copy_entry))
            .on_action(cx.listener(Self::toggle_pin))
            .on_action(cx.listener(Self::delete_entry))
            .on_mouse_move(cx.listener(Self::clip_drag_move))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::clip_drag_end))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::clip_drag_end))
            .on_action(cx.listener(Self::preview_down))
            .on_action(cx.listener(Self::preview_up))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(|this, _: &Run1, _, cx| this.run_row(0, cx)))
            .on_action(cx.listener(|this, _: &Run2, _, cx| this.run_row(1, cx)))
            .on_action(cx.listener(|this, _: &Run3, _, cx| this.run_row(2, cx)))
            .on_action(cx.listener(|this, _: &Run4, _, cx| this.run_row(3, cx)))
            .on_action(cx.listener(|this, _: &Run5, _, cx| this.run_row(4, cx)))
            .on_action(cx.listener(|this, _: &Run6, _, cx| this.run_row(5, cx)))
            .on_action(cx.listener(|this, _: &Run7, _, cx| this.run_row(6, cx)))
            .on_action(cx.listener(|this, _: &Run8, _, cx| this.run_row(7, cx)))
            .on_action(cx.listener(|this, _: &Run9, _, cx| this.run_row(8, cx)))
            .relative()
            .size_full()
            .rounded(radius)
            // Matte glass: the blurred screen, then the dark fill over it.
            .when_some(self.backdrop.clone(), |d, image| {
                d.child(layer().child(img(image).size_full().rounded(radius).object_fit(ObjectFit::Fill)))
            })
            .child(layer().bg(linear_gradient(
                180.,
                linear_color_stop(with_alpha(0x10181E, alpha), 0.),
                linear_color_stop(with_alpha(0x161222, (alpha + 0.04).min(1.)), 1.),
            )))
            .child(content)
            .child(layer().border_1().border_color(white(0.10)))
    }
}

impl Launcher {
    fn row(&self, i: usize, selected: bool, slot: usize, cx: &mut Context<Self>) -> AnyElement {
        let item = &self.hits[i].item;
        let hint: SharedString = if selected {
            "↵".into()
        } else if slot < 9 {
            shortcut(slot + 1).into()
        } else {
            "".into()
        };
        div()
            .id(("row", i))
            .h(px(62.))
            .flex_none()
            .px(px(14.))
            .flex()
            .items_center()
            .gap(px(14.))
            .rounded(px(12.))
            .border_1()
            .border_color(gpui::transparent_black())
            .when(!selected, |d| d.hover(|s| s.bg(white(0.04))))
            .when(selected, |d| {
                d.bg(linear_gradient(90., linear_color_stop(rgb(0xB3263A), 0.), linear_color_stop(rgb(0x8E1F33), 1.)))
                    .border_color(rgba(0xFF788273))
                    .shadow(vec![BoxShadow {
                        color: rgba(0xC81E3259).into(),
                        offset: point(px(0.), px(4.)),
                        blur_radius: px(18.),
                        spread_radius: px(0.),
                    }])
            })
            .on_click(cx.listener(move |this, _, _, cx| this.run_row(slot, cx)))
            .child(icon(item, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().text_size(px(19.)).font_weight(gpui::FontWeight::MEDIUM).truncate().child(item.title.clone()))
                    .child(
                        div()
                            .text_size(px(13.))
                            .truncate()
                            .text_color(if selected { white(0.80) } else { white(0.55) })
                            .child(item.subtitle.clone()),
                    ),
            )
            .child(div().text_size(px(15.)).text_color(if selected { white(0.9) } else { white(0.45) }).child(hint))
            .into_any_element()
    }
}

/// A 38 px tile: the app's own icon once loaded; until then (or without
/// one) a colored tile with the first letter. Built-in results get a glyph.
fn icon(item: &Item, cx: &gpui::App) -> AnyElement {
    if let (IconRef::App(id), Action::Launch(target)) = (&item.icon, &item.primary)
        && let Some(image) = crate::icons::get(cx, id, target)
    {
        return img(image).size(px(38.)).flex_none().object_fit(ObjectFit::Contain).into_any_element();
    }
    let (glyph, color): (String, u32) = match &item.icon {
        IconRef::Builtin(name) => {
            let g = match *name {
                "calculator" => "=",
                "folder" => "▸",
                "link" => "↗",
                "search" => "⌕",
                "gear" => "⚙",
                "refresh" => "↻",
                "info" => "i",
                "power" => "⏻",
                "snippet" => "✎",
                _ => "•",
            };
            (g.into(), 0x3A4152)
        }
        IconRef::App(id) => {
            let palette = [0xD9534F, 0xE67E22, 0xF1C40F, 0x27AE60, 0x16A085, 0x2E86DE, 0x8E44AD, 0xC0392B];
            let h = id.as_str().bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
            (item.title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), palette[h as usize % palette.len()])
        }
    };
    div()
        .size(px(38.))
        .flex_none()
        .rounded(px(9.))
        .bg(rgb(color))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(18.))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(white(0.95))
        .child(glyph)
        .into_any_element()
}

fn action_panel(panel: &ActionPanel) -> impl IntoElement {
    let labels = panel
        .actions
        .iter()
        .map(|a| match a {
            Action::CopyText(t) if *t == panel.item.title => "Copy name".to_string(),
            a => a.label().to_string(),
        })
        .collect();
    panel_view(panel.item.title.clone().into(), labels, panel.selected)
}

/// The Tab panel: a title and actions, the selected one highlighted.
pub(super) fn panel_view(title: SharedString, labels: Vec<String>, selected: usize) -> impl IntoElement {
    div()
        .absolute()
        .right(px(8.))
        .bottom(px(4.))
        .w(px(280.))
        .p(px(6.))
        .rounded(px(12.))
        .bg(rgba(0x12161FF5))
        .border_1()
        .border_color(white(0.12))
        .shadow(vec![BoxShadow {
            color: rgba(0x00000080).into(),
            offset: point(px(0.), px(8.)),
            blur_radius: px(24.),
            spread_radius: px(0.),
        }])
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().px(px(10.)).py(px(6.)).text_size(px(12.)).text_color(white(0.5)).truncate().child(title))
        .children(labels.into_iter().enumerate().map(|(i, label)| {
            div()
                .px(px(10.))
                .h(px(34.))
                .flex()
                .items_center()
                .rounded(px(8.))
                .text_size(px(14.))
                .when(i == selected, |d| d.bg(rgba(0xB3263AFF)))
                .child(label)
        }))
}

fn key_hint(key: &'static str, label: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .h(px(20.))
                .min_w(px(20.))
                .px(px(4.))
                .rounded(px(5.))
                .border_1()
                .border_color(white(0.18))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(white(0.75))
                .child(key),
        )
        .child(div().text_color(white(0.60)).child(label))
}

/// The RedSnap button: a red rounded square inside four corner brackets.
fn logo_button() -> impl IntoElement {
    let bracket = |top: bool, left: bool| {
        let d = div().absolute().size(px(8.)).border_color(white(0.85));
        let d = if top { d.top(px(11.)).border_t_2() } else { d.bottom(px(11.)).border_b_2() };
        if left { d.left(px(11.)).border_l_2() } else { d.right(px(11.)).border_r_2() }
    };
    div()
        .relative()
        .size(px(44.))
        .flex_none()
        .rounded(px(12.))
        .bg(white(0.06))
        .border_1()
        .border_color(white(0.08))
        .flex()
        .items_center()
        .justify_center()
        .child(bracket(true, true))
        .child(bracket(true, false))
        .child(bracket(false, true))
        .child(bracket(false, false))
        .child(div().size(px(12.)).rounded(px(3.)).bg(rgb(RED)))
}
