//! The settings pages. Each edit goes through `SettingsView::save` (or a
//! bound field), so RedSnap validates it before anything is written.

use std::path::PathBuf;

use gpui::components::*;
use gpui::{
    AnyElement, Context, ElementId, FontWeight, IntoElement, ParentElement, SharedString, Styled,
    Window, div, prelude::FluentBuilder, px, rgb, svg,
};
use redsnap_core::config::{Config, Providers, QuickLink, WebSearch};
use serde_json::json;

use super::view::{Page, RED, SettingsView, Shortcut};
use crate::assets::icon as icons;

type Cx<'a> = Context<'a, SettingsView>;

fn eid(s: String) -> ElementId {
    ElementId::Name(s.into())
}

pub fn render(v: &mut SettingsView, window: &mut Window, cx: &mut Cx) -> AnyElement {
    let _ = window;
    match v.page {
        Page::General => general(v, cx),
        Page::Search => search(v, cx),
        Page::Apps => apps(v, cx),
        Page::Snippets => snippets(v, cx),
        Page::Clipboard => clipboard(v, cx),
        Page::Advanced => advanced(v, cx),
    }
}

/// The RedSnap mark, `size` px: corner brackets around a red square.
pub fn mark(size: f32) -> impl IntoElement {
    div()
        .relative()
        .size(px(size))
        .flex_none()
        .child(
            svg()
                .absolute()
                .size(px(size))
                .path(icons::MARK_BRACKETS)
                .text_color(gpui::white().opacity(0.9)),
        )
        .child(
            svg()
                .absolute()
                .size(px(size))
                .path(icons::MARK_SQUARE)
                .text_color(rgb(RED)),
        )
}

/// A titled group of rows in a card.
fn group(title: &str, description: Option<&str>, rows: Vec<AnyElement>) -> AnyElement {
    section(title.to_string())
        .when_some(description.map(str::to_string), |s, d| s.description(d))
        .child(card().child(v_stack().divided().children(rows)))
        .into_any_element()
}

fn row(title: &str, description: Option<String>, control: impl IntoElement) -> AnyElement {
    setting_row(title.to_string())
        .when_some(description, |r, d| r.description(d))
        .control(control)
        .into_any_element()
}

fn note(text: impl Into<SharedString>, cx: &Cx) -> AnyElement {
    div()
        .px(px(18.))
        .py(px(12.))
        .text_size(px(12.5))
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

const RESTART: &str = " · applies after RedSnap restarts";

fn start_label() -> &'static str {
    if cfg!(windows) {
        "Start with Windows"
    } else {
        "Start at login"
    }
}

/// What `f` searches on this OS.
fn file_index() -> &'static str {
    if cfg!(windows) {
        "uses Everything (voidtools.com)"
    } else if cfg!(target_os = "macos") {
        "uses Spotlight"
    } else {
        "uses plocate"
    }
}

/// A new id for a list item: the name as a slug and a few random characters.
fn new_id(name: &str) -> String {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-');
    let slug = if slug.is_empty() { "item" } else { slug };
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{slug}-{:05x}", nanos & 0xFFFFF)
}

fn toggle_row(
    v: &SettingsView,
    id: &'static str,
    title: &str,
    description: Option<String>,
    checked: bool,
    cx: &mut Cx,
    set: fn(&mut Config, bool),
) -> AnyElement {
    let _ = v;
    row(
        title,
        description,
        toggle(id, checked).on_change(cx.listener(move |this, on: &bool, _, cx| {
            let on = *on;
            this.save(cx, move |c| set(c, on));
        })),
    )
}

/// A number field: digits only, within `range`.
fn number(
    v: &mut SettingsView,
    key: &str,
    range: std::ops::RangeInclusive<u32>,
    cx: &mut Cx,
    get: fn(&Config) -> u32,
    set: fn(&mut Config, u32),
) -> AnyElement {
    let (lo, hi) = (*range.start(), *range.end());
    let state = v.line(
        key,
        "",
        cx,
        move |c| Some(get(c).to_string()),
        move |c, s| {
            let n: u32 = s
                .trim()
                .parse()
                .map_err(|_| format!("Type a number from {lo} to {hi}"))?;
            if !(lo..=hi).contains(&n) {
                return Err(format!("Type a number from {lo} to {hi}"));
            }
            set(c, n);
            Ok(())
        },
    );
    input(&state).w(px(110.)).into_any_element()
}

/// A list of strings with Remove buttons and a row to add one.
#[allow(clippy::too_many_arguments)]
fn string_list(
    v: &mut SettingsView,
    key: &'static str,
    items: Vec<String>,
    placeholder: &str,
    empty: &str,
    cx: &mut Cx,
    set: fn(&mut Config, Vec<String>),
) -> Vec<AnyElement> {
    let mut rows: Vec<AnyElement> = Vec::new();
    if items.is_empty() {
        rows.push(note(empty.to_string(), cx));
    }
    for (i, item) in items.iter().enumerate() {
        let rest: Vec<String> = items
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, s)| s.clone())
            .collect();
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(18.))
                .py(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .truncate()
                        .child(item.clone()),
                )
                .child(
                    button((key, i))
                        .label("Remove")
                        .ghost()
                        .size(ComponentSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let rest = rest.clone();
                            this.save(cx, move |c| set(c, rest));
                        })),
                )
                .into_any_element(),
        );
    }
    let draft_key = format!("{key}:draft");
    let add = {
        let items = items.clone();
        let draft_key = draft_key.clone();
        move |this: &mut SettingsView, cx: &mut Cx| {
            let value = this.plain_value(&draft_key, cx);
            if value.is_empty() || items.contains(&value) {
                return;
            }
            let mut next = items.clone();
            next.push(value);
            if let Some((s, _)) = this.inputs.get(&draft_key) {
                s.update(cx, |s, cx| s.clear(cx));
            }
            this.save(cx, move |c| set(c, next));
        }
    };
    let add_on_enter = add.clone();
    let draft = v.plain(&draft_key, placeholder, cx, move |this, _, e, cx| {
        if matches!(e, InputEvent::Submit(_)) {
            add_on_enter(this, cx);
        }
    });
    rows.push(
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(18.))
            .py(px(10.))
            .child(div().flex_1().child(input(&draft)))
            .child(
                button(eid(format!("{key}-add")))
                    .label("Add")
                    .secondary()
                    .size(ComponentSize::Sm)
                    .on_click(cx.listener(move |this, _, _, cx| add(this, cx))),
            )
            .into_any_element(),
    );
    rows
}

// ----- General --------------------------------------------------------------------

fn general(v: &mut SettingsView, cx: &mut Cx) -> AnyElement {
    let c = v.snap.as_ref().unwrap().config.clone();
    let shortcuts = group(
        "Shortcuts",
        Some("Click a shortcut, then press the keys you want."),
        vec![
            row(
                "Open RedSnap",
                Some("Press it again to hide RedSnap.".into()),
                v.recorder(Shortcut::Launcher, &c.hotkey, cx),
            ),
            row(
                "Open clipboard history",
                Some("Lists everything you've copied.".into()),
                v.recorder(Shortcut::Clipboard, &c.clipboard.hotkey, cx),
            ),
        ],
    );
    let startup = group(
        "Startup",
        None,
        vec![toggle_row(
            v,
            "start-at-login",
            start_label(),
            Some("RedSnap starts in the background, ready for its shortcut.".into()),
            c.start_at_login,
            cx,
            |c, on| c.start_at_login = on,
        )],
    );
    let rows = number(
        v,
        "rows",
        1..=12,
        cx,
        |c| c.rows as u32,
        |c, n| c.rows = n as u8,
    );
    let window = group(
        "Window",
        None,
        vec![row(
            "Visible results",
            Some(format!("Rows in the search window, 1 to 12{RESTART}")),
            rows,
        )],
    );
    v_stack()
        .spacing(px(28.))
        .child(shortcuts)
        .child(startup)
        .child(window)
        .into_any_element()
}

// ----- Search --------------------------------------------------------------------

type ProviderFlag = fn(&mut Providers) -> &mut bool;

fn search(v: &mut SettingsView, cx: &mut Cx) -> AnyElement {
    let c = v.snap.as_ref().unwrap().config.clone();
    let p = &c.providers;
    let file_hint = format!("f and a name: f report · {}", file_index());
    let providers: [(&'static str, &str, String, bool, ProviderFlag); 8] = [
        (
            "p-apps",
            "Apps",
            "Installed applications".into(),
            p.apps,
            |p| &mut p.apps,
        ),
        (
            "p-calc",
            "Calculator",
            "Type an expression like 2*(3+4)".into(),
            p.calculator,
            |p| &mut p.calculator,
        ),
        (
            "p-web",
            "Web searches",
            "A keyword and words: g rust".into(),
            p.web_searches,
            |p| &mut p.web_searches,
        ),
        (
            "p-links",
            "Quick links",
            "Your saved links".into(),
            p.quick_links,
            |p| &mut p.quick_links,
        ),
        (
            "p-open",
            "Open URLs and browse folders",
            "Type github.com, or a path like C:\\Users\\ to list it".into(),
            p.open,
            |p| &mut p.open,
        ),
        ("p-files", "File search", file_hint, p.files, |p| {
            &mut p.files
        }),
        (
            "p-cmds",
            "RedSnap commands",
            "Settings, Reload Apps, Quit…".into(),
            p.commands,
            |p| &mut p.commands,
        ),
        (
            "p-sys",
            "System commands",
            "Lock, Sleep, Restart, Shut Down…".into(),
            p.system_commands,
            |p| &mut p.system_commands,
        ),
    ];
    let rows = providers
        .into_iter()
        .map(|(id, title, hint, on, flag)| {
            row(
                title,
                Some(hint),
                toggle(id, on).on_change(cx.listener(move |this, on: &bool, _, cx| {
                    let on = *on;
                    this.save(cx, move |c| *flag(&mut c.providers) = on);
                })),
            )
        })
        .collect();
    let what = group("What search finds", None, rows);
    let web = link_table(
        v,
        "ws",
        "Web searches",
        "Type the keyword, a space and your words. {query} in the URL is replaced by them.",
        c.web_searches
            .iter()
            .map(|w| {
                (
                    w.id.clone(),
                    w.name.clone(),
                    w.keyword.clone(),
                    w.url.clone(),
                )
            })
            .collect(),
        true,
        cx,
    );
    let links = link_table(
        v,
        "ql",
        "Quick links",
        "Found by name or keyword. With {query} in the URL, they take words like a web search.",
        c.quick_links
            .iter()
            .map(|l| {
                (
                    l.id.clone(),
                    l.name.clone(),
                    l.keyword.clone(),
                    l.url.clone(),
                )
            })
            .collect(),
        false,
        cx,
    );
    v_stack()
        .spacing(px(28.))
        .child(what)
        .child(web)
        .child(links)
        .into_any_element()
}

/// One link's name, keyword or URL, by id.
fn link_get(c: &Config, kind: &str, id: &str, field: usize) -> Option<String> {
    let pick = |name: &String, keyword: &String, url: &String| match field {
        0 => name.clone(),
        1 => keyword.clone(),
        _ => url.clone(),
    };
    if kind == "ws" {
        c.web_searches
            .iter()
            .find(|w| w.id == id)
            .map(|w| pick(&w.name, &w.keyword, &w.url))
    } else {
        c.quick_links
            .iter()
            .find(|l| l.id == id)
            .map(|l| pick(&l.name, &l.keyword, &l.url))
    }
}

fn link_field<'a>(c: &'a mut Config, kind: &str, id: &str, field: usize) -> Option<&'a mut String> {
    let pick = |name: &'a mut String, keyword: &'a mut String, url: &'a mut String| match field {
        0 => name,
        1 => keyword,
        _ => url,
    };
    if kind == "ws" {
        c.web_searches
            .iter_mut()
            .find(|w| w.id == id)
            .map(|w| pick(&mut w.name, &mut w.keyword, &mut w.url))
    } else {
        c.quick_links
            .iter_mut()
            .find(|l| l.id == id)
            .map(|l| pick(&mut l.name, &mut l.keyword, &mut l.url))
    }
}

fn link_table(
    v: &mut SettingsView,
    kind: &'static str,
    title: &str,
    description: &str,
    items: Vec<(String, String, String, String)>,
    needs_query: bool,
    cx: &mut Cx,
) -> AnyElement {
    let mut rows: Vec<AnyElement> = Vec::new();
    if items.is_empty() {
        rows.push(note("None yet.", cx));
    }
    for (id, ..) in &items {
        let field = |v: &mut SettingsView, n: usize, placeholder: &str, cx: &mut Cx| {
            let (id_get, id_set) = (id.clone(), id.clone());
            v.line(
                &format!("{kind}:{id}:{n}"),
                placeholder,
                cx,
                move |c| link_get(c, kind, &id_get, n),
                move |c, s| {
                    if let Some(f) = link_field(c, kind, &id_set, n) {
                        *f = s.trim().to_string();
                    }
                    Ok(())
                },
            )
        };
        let name = field(v, 0, "Name", cx);
        let keyword = field(v, 1, "Keyword", cx);
        let url = field(v, 2, "https://…{query}", cx);
        let remove_id = id.clone();
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(18.))
                .py(px(10.))
                .child(input(&name).w(px(150.)))
                .child(input(&keyword).w(px(90.)))
                .child(div().flex_1().min_w_0().child(input(&url)))
                .child(
                    button(eid(format!("{kind}-remove-{id}")))
                        .label("Remove")
                        .ghost()
                        .size(ComponentSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = remove_id.clone();
                            this.save(cx, move |c| {
                                if kind == "ws" {
                                    c.web_searches.retain(|w| w.id != id);
                                } else {
                                    c.quick_links.retain(|l| l.id != id);
                                }
                            });
                        })),
                )
                .into_any_element(),
        );
    }
    let title_owned = title.to_string();
    rows.push(
        div()
            .px(px(18.))
            .py(px(10.))
            .child(
                button(eid(format!("{kind}-add")))
                    .label("Add")
                    .secondary()
                    .size(ComponentSize::Sm)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = new_id(&title_owned);
                        this.save(cx, move |c| {
                            let url = if needs_query {
                                "https://example.com/search?q={query}"
                            } else {
                                "https://example.com"
                            };
                            if kind == "ws" {
                                c.web_searches.push(WebSearch {
                                    id,
                                    name: "New".into(),
                                    keyword: String::new(),
                                    url: url.into(),
                                });
                            } else {
                                c.quick_links.push(QuickLink {
                                    id,
                                    name: "New".into(),
                                    keyword: String::new(),
                                    url: url.into(),
                                });
                            }
                        });
                    })),
            )
            .into_any_element(),
    );
    group(title, Some(description), rows)
}

// ----- Apps ----------------------------------------------------------------------

fn apps(v: &mut SettingsView, cx: &mut Cx) -> AnyElement {
    let c = v.snap.as_ref().unwrap().config.clone();
    let stats = v.stats.clone();
    let description = match &stats {
        Some(s) => {
            let mut d = format!("{} apps found", s["count"].as_u64().unwrap_or(0));
            if s["scanning"].as_bool() == Some(true) {
                d.push_str(" · scanning…");
            }
            d
        }
        None => "Loading…".into(),
    };
    let all: Vec<(String, String)> = stats
        .as_ref()
        .and_then(|s| s["apps"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .map(|a| {
            (
                a["id"].as_str().unwrap_or("").to_string(),
                a["name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let filter_input = v.plain("apps:filter", "Filter apps", cx, |_, _, e, cx| {
        if matches!(e, InputEvent::Change(_)) {
            cx.notify();
        }
    });
    let filter = v.plain_value("apps:filter", cx).to_lowercase();
    let mut rows: Vec<AnyElement> = vec![
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(18.))
            .py(px(12.))
            .child(div().flex_1().child(input(&filter_input)))
            .child(
                button("scan")
                    .label("Scan again")
                    .secondary()
                    .size(ComponentSize::Sm)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.call("apps.reload", json!({}), cx, |_, _, _, _| {});
                    })),
            )
            .into_any_element(),
    ];
    let shown: Vec<&(String, String)> = all
        .iter()
        .filter(|(_, n)| filter.is_empty() || n.to_lowercase().contains(&filter))
        .take(200)
        .collect();
    for (id, name) in shown {
        let hidden = c.hidden_apps.contains(id);
        let (get_id, set_id) = (id.clone(), id.clone());
        let aliases = v.line(
            &format!("alias:{id}"),
            "Also find by…",
            cx,
            move |c| {
                Some(
                    c.app_aliases
                        .get(&get_id)
                        .map(|a| a.join(", "))
                        .unwrap_or_default(),
                )
            },
            move |c, s| {
                let list: Vec<String> = s
                    .split(',')
                    .map(|a| a.trim().to_string())
                    .filter(|a| !a.is_empty())
                    .collect();
                if list.is_empty() {
                    c.app_aliases.remove(&set_id);
                } else {
                    c.app_aliases.insert(set_id.clone(), list);
                }
                Ok(())
            },
        );
        let toggle_id = id.clone();
        let theme = cx.theme();
        rows.push(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(18.))
                .py(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.5))
                        .when(hidden, |d| {
                            d.line_through().text_color(theme.muted_foreground)
                        })
                        .child(name.clone()),
                )
                .child(input(&aliases).w(px(220.)))
                .child(
                    button(eid(format!("hide-{id}")))
                        .label(if hidden { "Show" } else { "Hide" })
                        .ghost()
                        .size(ComponentSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = toggle_id.clone();
                            this.save(cx, move |c| {
                                if c.hidden_apps.contains(&id) {
                                    c.hidden_apps.retain(|h| *h != id);
                                } else {
                                    c.hidden_apps.push(id);
                                }
                            });
                        })),
                )
                .into_any_element(),
        );
    }
    let installed = group("Installed apps", Some(&description), rows);
    let names: std::collections::HashMap<&str, &str> =
        all.iter().map(|(i, n)| (i.as_str(), n.as_str())).collect();
    let hidden = (!c.hidden_apps.is_empty()).then(|| {
        let rows = c
            .hidden_apps
            .iter()
            .map(|id| {
                let show_id = id.clone();
                row(
                    names.get(id.as_str()).copied().unwrap_or(id),
                    None,
                    button(eid(format!("show-{id}")))
                        .label("Show again")
                        .ghost()
                        .size(ComponentSize::Sm)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let id = show_id.clone();
                            this.save(cx, move |c| c.hidden_apps.retain(|h| *h != id));
                        })),
                )
            })
            .collect();
        group("Hidden apps", Some("Left out of search."), rows)
    });
    let folders: Vec<String> = c
        .extra_app_folders
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let folder_rows = string_list(
        v,
        "folders",
        folders,
        "A folder path",
        "Only the usual places.",
        cx,
        |c, items| {
            c.extra_app_folders = items.into_iter().map(PathBuf::from).collect();
        },
    );
    let more = group(
        "More app folders",
        Some("Searched for apps in addition to the usual places."),
        folder_rows,
    );
    v_stack()
        .spacing(px(28.))
        .child(installed)
        .children(hidden)
        .child(more)
        .into_any_element()
}

// ----- Snippets ------------------------------------------------------------------

fn snippets(v: &mut SettingsView, cx: &mut Cx) -> AnyElement {
    let c = v.snap.as_ref().unwrap().config.clone();
    let s = &c.snippets;
    let expand_hint = if cfg!(windows) {
        "Typing a trigger replaces it with its snippet."
    } else {
        "Not available on this system yet; snippets still work from RedSnap."
    };
    let help = "A trigger that starts with a letter expands only at the start of a word; triggers starting with a symbol, like ;sig, \
                expand anywhere. Placeholders: {{cursor}} where the caret ends up, {{date}}, {{time}}, {{datetime}}, \
                {{date:%d/%m/%Y}} any date format, {{clipboard}} the clipboard's text.";
    let top = group(
        "Snippets",
        Some(
            "Text you use often. Type a snippet's trigger in any app, or find it in RedSnap and press Enter.",
        ),
        vec![
            toggle_row(
                v,
                "expand",
                "Expand as you type",
                Some(expand_hint.into()),
                s.expand_as_you_type,
                cx,
                |c, on| c.snippets.expand_as_you_type = on,
            ),
            note(help, cx),
        ],
    );
    let filter_input = v.plain("snippets:filter", "Filter snippets", cx, |_, _, e, cx| {
        if matches!(e, InputEvent::Change(_)) {
            cx.notify();
        }
    });
    let filter = v.plain_value("snippets:filter", cx).to_lowercase();
    let toolbar = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_1().child(input(&filter_input)))
        .child(
            button("add-snippet")
                .label("Add snippet")
                .size(ComponentSize::Sm)
                .on_click(cx.listener(|this, _, _, cx| {
                    let id = new_id("snippet");
                    this.save(cx, move |c| {
                        c.snippets.items.push(redsnap_core::config::Snippet {
                            id,
                            name: "New snippet".into(),
                            trigger: String::new(),
                            text: String::new(),
                            auto_expand: true,
                        })
                    });
                })),
        );
    let shown: Vec<_> = s
        .items
        .iter()
        .filter(|i| {
            filter.is_empty()
                || format!("{} {} {}", i.name, i.trigger, i.text)
                    .to_lowercase()
                    .contains(&filter)
        })
        .cloned()
        .collect();
    let mut list: Vec<AnyElement> = Vec::new();
    if shown.is_empty() {
        let theme = cx.theme();
        list.push(
            div()
                .text_size(px(13.))
                .text_color(theme.muted_foreground)
                .child(if s.items.is_empty() {
                    "No snippets yet."
                } else {
                    "No snippet matches."
                })
                .into_any_element(),
        );
    }
    for item in shown {
        let id = item.id.clone();
        let (g1, s1, g2, s2, g3, s3) = (
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
            id.clone(),
        );
        let name = v.line(
            &format!("sn:{id}:name"),
            "Name",
            cx,
            move |c| {
                c.snippets
                    .items
                    .iter()
                    .find(|i| i.id == g1)
                    .map(|i| i.name.clone())
            },
            move |c, v| {
                if let Some(i) = c.snippets.items.iter_mut().find(|i| i.id == s1) {
                    i.name = v.to_string();
                }
                Ok(())
            },
        );
        let trigger = v.line(
            &format!("sn:{id}:trigger"),
            "Trigger, e.g. ;sig",
            cx,
            move |c| {
                c.snippets
                    .items
                    .iter()
                    .find(|i| i.id == g2)
                    .map(|i| i.trigger.clone())
            },
            move |c, v| {
                if let Some(i) = c.snippets.items.iter_mut().find(|i| i.id == s2) {
                    i.trigger = v.trim().to_string();
                }
                Ok(())
            },
        );
        let text = v.area(
            &format!("sn:{id}:text"),
            "The text. Ctrl+Enter to save.",
            cx,
            move |c| {
                c.snippets
                    .items
                    .iter()
                    .find(|i| i.id == g3)
                    .map(|i| i.text.clone())
            },
            move |c, v| {
                if let Some(i) = c.snippets.items.iter_mut().find(|i| i.id == s3) {
                    i.text = v.to_string();
                }
                Ok(())
            },
        );
        let (toggle_id, remove_id) = (id.clone(), id.clone());
        let theme = cx.theme();
        list.push(
            card()
                .padded()
                .child(
                    v_stack()
                        .spacing(px(12.))
                        .child(
                            h_stack()
                                .spacing(px(8.))
                                .child(input(&name).w(px(230.)))
                                .child(input(&trigger).w(px(150.)))
                                .child(div().flex_1())
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .text_color(theme.muted_foreground)
                                        .child("Expand as typed"),
                                )
                                .child(
                                    toggle(eid(format!("auto-{id}")), item.auto_expand).on_change(
                                        cx.listener(move |this, on: &bool, _, cx| {
                                            let (on, id) = (*on, toggle_id.clone());
                                            this.save(cx, move |c| {
                                                if let Some(i) =
                                                    c.snippets.items.iter_mut().find(|i| i.id == id)
                                                {
                                                    i.auto_expand = on;
                                                }
                                            });
                                        }),
                                    ),
                                )
                                .child(
                                    button(eid(format!("remove-{id}")))
                                        .label("Remove")
                                        .ghost()
                                        .size(ComponentSize::Sm)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let id = remove_id.clone();
                                            this.save(cx, move |c| {
                                                c.snippets.items.retain(|i| i.id != id)
                                            });
                                        })),
                                ),
                        )
                        .child(text_area(&text)),
                )
                .into_any_element(),
        );
    }
    let excluded_rows = string_list(
        v,
        "sn-excluded",
        s.excluded_apps.clone(),
        if cfg!(windows) {
            "WindowsTerminal.exe"
        } else {
            "Terminal"
        },
        "No apps excluded.",
        cx,
        |c, items| c.snippets.excluded_apps = items,
    );
    let excluded = group(
        "Excluded apps",
        Some(
            "Triggers never expand in these apps (executable or app name). Password fields are always skipped.",
        ),
        excluded_rows,
    );
    v_stack()
        .spacing(px(28.))
        .child(top)
        .child(v_stack().spacing(px(12.)).child(toolbar).children(list))
        .child(excluded)
        .into_any_element()
}

// ----- Clipboard -----------------------------------------------------------------

fn clipboard(v: &mut SettingsView, cx: &mut Cx) -> AnyElement {
    let c = v.snap.as_ref().unwrap().config.clone();
    let cb = &c.clipboard;
    let history = group(
        "Clipboard history",
        Some("Every copy is either kept or listed as not kept, with the reason."),
        vec![
            toggle_row(
                v,
                "cb-enabled",
                "Keep clipboard history",
                Some(format!("Off: nothing is recorded{RESTART}")),
                cb.enabled,
                cx,
                |c, on| c.clipboard.enabled = on,
            ),
            row(
                "Shortcut",
                None,
                v.recorder(Shortcut::Clipboard, &cb.hotkey, cx),
            ),
            toggle_row(
                v,
                "cb-skipped",
                "Show copies that weren't kept",
                Some(
                    "Dimmed, with the reason (marked private, too large, an excluded app…).".into(),
                ),
                cb.show_skipped,
                cx,
                |c, on| c.clipboard.show_skipped = on,
            ),
        ],
    );
    let days = number(
        v,
        "keep-days",
        1..=3650,
        cx,
        |c| c.clipboard.keep_days,
        |c, n| c.clipboard.keep_days = n,
    );
    let max = number(
        v,
        "max-entries",
        10..=100_000,
        cx,
        |c| c.clipboard.max_entries,
        |c, n| c.clipboard.max_entries = n,
    );
    let entries = match &v.clip_stats {
        Some(s) if s["available"].as_bool() == Some(true) => {
            format!("{} entries", s["entries"].as_u64().unwrap_or(0))
        }
        Some(_) => "Not running".into(),
        None => String::new(),
    };
    let clear = v.confirm_button(
        "clear-clipboard",
        "Clear history",
        "Click again to clear (pinned stay)",
        cx,
        |this, cx| {
            this.call("clipboard.clear", json!({}), cx, |this, r, _, cx| match r {
                Ok(_) => {
                    this.refresh_clip_stats(cx);
                    this.notify(false, "Clipboard history cleared; pinned entries kept", cx);
                }
                Err(e) => this.notify(true, e, cx),
            });
        },
    );
    let keeping = group(
        "Keeping",
        Some(&format!("Pinned entries are always kept{RESTART}.")),
        vec![
            row("Keep entries for", Some("Days".into()), days),
            row("Keep at most", Some("Entries".into()), max),
            row("History", Some(entries), clear),
        ],
    );
    let excluded_rows = string_list(
        v,
        "cb-excluded",
        cb.excluded_apps.clone(),
        if cfg!(windows) {
            "KeePassXC.exe"
        } else {
            "1Password"
        },
        "No apps excluded. Apps that mark copies private (password managers) are skipped anyway.",
        cx,
        |c, items| c.clipboard.excluded_apps = items,
    );
    let excluded = group(
        "Excluded apps",
        Some("Copies from these apps are listed as not kept. Use the app's executable or name."),
        excluded_rows,
    );
    v_stack()
        .spacing(px(28.))
        .child(history)
        .child(keeping)
        .child(excluded)
        .into_any_element()
}

// ----- Advanced ------------------------------------------------------------------

fn advanced(v: &mut SettingsView, cx: &mut Cx) -> AnyElement {
    let info = v.info.clone().unwrap_or_default();
    let text = |key: &str| info[key].as_str().map(str::to_string);
    let reset = v.confirm_button(
        "reset-usage",
        "Reset",
        "Click again to reset",
        cx,
        |this, cx| {
            this.call("usage.reset", json!({}), cx, |this, r, _, cx| match r {
                Ok(_) => this.notify(false, "Learned ranking reset", cx),
                Err(e) => this.notify(true, e, cx),
            });
        },
    );
    let learning = group(
        "Learning",
        None,
        vec![row(
            "Forget what RedSnap learned",
            Some("Which results you picked for which words.".into()),
            reset,
        )],
    );
    let open = |id: &'static str, method: &'static str, cx: &mut Cx| {
        button(id)
            .label("Open folder")
            .secondary()
            .size(ComponentSize::Sm)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.call(method, json!({}), cx, |this, r, _, cx| {
                    if let Err(e) = r {
                        this.notify(true, e, cx);
                    }
                });
            }))
    };
    let theme = cx.theme();
    let muted = |s: &str| {
        div()
            .text_size(px(12.))
            .text_color(theme.muted_foreground)
            .child(s.to_string())
    };
    let files = group(
        "Files",
        None,
        vec![
            row(
                "Logs",
                Some("For reporting a problem.".into()),
                open("logs", "logs.open", cx),
            ),
            row("Data", text("dataDir"), open("data", "data.open", cx)),
            row("Settings file", text("configDir"), muted("config.json")),
        ],
    );
    let about = group(
        "About",
        None,
        vec![row(
            "RedSnap",
            text("platform"),
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::MEDIUM)
                .child(text("version").unwrap_or_default()),
        )],
    );
    v_stack()
        .spacing(px(28.))
        .child(learning)
        .child(files)
        .child(about)
        .into_any_element()
}
