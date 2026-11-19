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
                                .child(icon_svg(crate::assets::icon::X, 14., white(0.8)))
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
        let enter_label: &'static str = if in_clip {
            "Paste"
        } else {
            self.hits.get(self.selected).map(|h| h.item.primary.label()).filter(|l| !l.is_empty()).unwrap_or("Open")
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
            .child(key_hint(Cap::Icon(crate::assets::icon::ARROW_UP_DOWN), "Navigate"))
            .child(div().w(px(16.)))
            .child(key_hint(Cap::Icon(crate::assets::icon::CORNER_DOWN_LEFT), enter_label))
            .child(div().w(px(16.)))
            .when(in_clip, |d| d.child(key_hint(Cap::Text(copy_key), "Copy")).child(div().w(px(16.))))
            .child(key_hint(Cap::Icon(crate::assets::icon::ARROW_RIGHT_TO_LINE), "Actions"));

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
        let hint: AnyElement = if selected {
            icon_svg(crate::assets::icon::CORNER_DOWN_LEFT, 17., white(0.9)).into_any_element()
        } else if slot < 9 {
            div().text_size(px(15.)).text_color(white(0.45)).child(SharedString::from(shortcut(slot + 1))).into_any_element()
        } else {
            div().into_any_element()
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
            .child(hint)
            .into_any_element()
    }
}

/// A 38 px tile: the app's own icon once loaded; until then (or without
/// one) a colored tile with the first letter. Built-in results get an icon.
fn icon(item: &Item, cx: &gpui::App) -> AnyElement {
    use crate::assets::icon as i;
    if let (IconRef::App(id), Action::Launch(target)) = (&item.icon, &item.primary)
        && let Some(image) = crate::icons::get(cx, id, target)
    {
        return img(image).size(px(38.)).flex_none().object_fit(ObjectFit::Contain).into_any_element();
    }
    let tile = |color: u32| div().size(px(38.)).flex_none().rounded(px(9.)).bg(rgb(color)).flex().items_center().justify_center();
    match &item.icon {
        IconRef::Builtin(name) => {
            let path = match *name {
                "calculator" => i::CALCULATOR,
                "folder" => i::FOLDER,
                "file" => i::FILE,
                "link" => i::EXTERNAL_LINK,
                "search" => i::SEARCH,
                "gear" => i::SETTINGS,
                "refresh" => i::REFRESH_CW,
                "info" => i::INFO,
                "power" => i::POWER,
                "snippet" => i::SQUARE_PEN,
                _ => i::INFO,
            };
            tile(0x3A4152).child(icon_svg(path, 20., white(0.92))).into_any_element()
        }
        IconRef::App(id) => {
            let palette = [0xD9534F, 0xE67E22, 0xF1C40F, 0x27AE60, 0x16A085, 0x2E86DE, 0x8E44AD, 0xC0392B];
            let h = id.as_str().bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
            let letter = item.title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            tile(palette[h as usize % palette.len()])
                .text_size(px(18.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(white(0.95))
                .child(letter)
                .into_any_element()
        }
    }
}

/// The file or folder a result is about, if any.
pub(super) fn panel_path(item: &Item) -> Option<&std::path::Path> {
    [Some(&item.primary), item.secondary.as_ref()].into_iter().flatten().find_map(|a| match a {
        Action::OpenPath(p) | Action::RevealPath(p) => Some(p.as_path()),
        _ => None,
    })
}

fn action_panel(panel: &ActionPanel) -> impl IntoElement {
    let labels = panel
        .actions
        .iter()
        .map(|a| match a {
            Action::CopyText(t) if *t == panel.item.title => "Copy name".to_string(),
            Action::CopyText(t) if panel_path(&panel.item).is_some_and(|p| p.display().to_string() == *t) => "Copy path".to_string(),
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

/// What a footer key cap shows.
pub(super) enum Cap {
    Icon(&'static str),
    Text(&'static str),
}

/// A Lucide icon (assets.rs), tinted with `color`.
pub(super) fn icon_svg(path: &'static str, size: f32, color: Hsla) -> gpui::Svg {
    gpui::svg().path(path).size(px(size)).flex_none().text_color(color)
}

pub(super) fn key_hint(cap: Cap, label: &'static str) -> impl IntoElement {
    let cap_el: AnyElement = match cap {
        Cap::Icon(path) => icon_svg(path, 12., white(0.8)).into_any_element(),
        Cap::Text(t) => div().text_size(px(11.)).text_color(white(0.75)).child(t).into_any_element(),
    };
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
                .child(cap_el),
        )
        .child(div().text_color(white(0.60)).child(label))
}

/// The RedSnap mark, filling its 44 px square: corner brackets around a red
/// rounded square, no frame. Drawn as two SVG layers over the same box, so
/// they're centered on each other at any display scale (separate boxes
/// rounded apart at 125%, leaving the square a pixel or two high).
fn logo_button() -> impl IntoElement {
    let layer = |path: &'static str, color: Hsla| {
        gpui::svg().path(path).absolute().top_0().left_0().size_full().text_color(color)
    };
    div()
        .relative()
        .size(px(44.))
        .flex_none()
        .child(layer(crate::assets::icon::MARK_BRACKETS, white(0.9)))
        .child(layer(crate::assets::icon::MARK_SQUARE, rgb(RED).into()))
}
