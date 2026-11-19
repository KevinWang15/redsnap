//! Snippets: templates, and the matcher that finds a trigger in what was just
//! typed. Expansion itself (watching keys, typing the result) is per OS, in
//! `redsnap_platform::snippets`.
//!
//! Template placeholders:
//! - `{{cursor}}`: where the caret ends up (once);
//! - `{{date}}` `2026-11-19`, `{{time}}` `14:05`, `{{datetime}}` `2026-11-19 14:05`;
//! - `{{date:FORMAT}}`: any strftime format, e.g. `{{date:%d/%m/%Y}}`;
//! - `{{clipboard}}`: the clipboard's text;
//! - `\{{` for a literal `{{`.

use chrono::{DateTime, Local};

use crate::config::Snippet;

/// What a template needs from the moment it's expanded.
pub struct Context {
    pub now: DateTime<Local>,
    /// The clipboard's text, if it has any.
    pub clipboard: Option<String>,
}

/// An expanded snippet: the text, and how many characters from its end the
/// caret goes (0: at the end).
#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub cursor_back: usize,
}

const MAX_FORMAT: usize = 64;

/// Checks a template's placeholders; the error names the first bad one.
pub fn check(template: &str) -> Result<(), String> {
    let mut cursors = 0;
    for token in tokens(template) {
        match token {
            Token::Text(_) => {}
            Token::Placeholder(p) => match p {
                "cursor" => cursors += 1,
                "date" | "time" | "datetime" | "clipboard" => {}
                p if p.starts_with("date:") => {
                    let f = &p[5..];
                    if f.is_empty() || f.len() > MAX_FORMAT {
                        return Err(format!(
                            "{{{{{p}}}}}: the date format must be 1 to {MAX_FORMAT} characters"
                        ));
                    }
                    if chrono::format::StrftimeItems::new(f)
                        .any(|i| matches!(i, chrono::format::Item::Error))
                    {
                        return Err(format!("{{{{{p}}}}}: {f:?} isn't a valid date format"));
                    }
                }
                other => {
                    return Err(format!(
                        "{{{{{other}}}}} isn't a placeholder (cursor, date, time, datetime, date:FORMAT, clipboard)"
                    ));
                }
            },
            Token::Unclosed => return Err("a {{ without its }}".into()),
        }
    }
    if cursors > 1 {
        return Err("{{cursor}} can appear only once".into());
    }
    Ok(())
}

pub fn render(template: &str, cx: &Context) -> Rendered {
    let mut text = String::new();
    let mut cursor_at: Option<usize> = None;
    for token in tokens(template) {
        match token {
            Token::Text(t) => text.push_str(&t),
            Token::Placeholder("cursor") => cursor_at = Some(text.chars().count()),
            Token::Placeholder("date") => text.push_str(&cx.now.format("%Y-%m-%d").to_string()),
            Token::Placeholder("time") => text.push_str(&cx.now.format("%H:%M").to_string()),
            Token::Placeholder("datetime") => {
                text.push_str(&cx.now.format("%Y-%m-%d %H:%M").to_string())
            }
            Token::Placeholder("clipboard") => text.push_str(cx.clipboard.as_deref().unwrap_or("")),
            Token::Placeholder(p) if p.starts_with("date:") => {
                // Formats are checked on save; a bad one renders as written.
                use std::fmt::Write;
                let mut s = String::new();
                if write!(s, "{}", cx.now.format(&p[5..])).is_ok() {
                    text.push_str(&s);
                }
            }
            Token::Placeholder(p) => {
                text.push_str("{{");
                text.push_str(p);
                text.push_str("}}");
            }
            Token::Unclosed => text.push_str("{{"),
        }
    }
    let total = text.chars().count();
    Rendered {
        cursor_back: cursor_at.map_or(0, |at| total - at),
        text,
    }
}

enum Token<'a> {
    Text(String),
    Placeholder(&'a str),
    /// `{{` with no `}}` after it; the rest is text.
    Unclosed,
}

fn tokens(t: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = t;
    while !rest.is_empty() {
        if let Some(r) = rest.strip_prefix("\\{{") {
            text.push_str("{{");
            rest = r;
        } else if let Some(r) = rest.strip_prefix("{{") {
            match r.find("}}") {
                Some(end) => {
                    if !text.is_empty() {
                        out.push(Token::Text(std::mem::take(&mut text)));
                    }
                    out.push(Token::Placeholder(r[..end].trim()));
                    rest = &r[end + 2..];
                }
                None => {
                    if !text.is_empty() {
                        out.push(Token::Text(std::mem::take(&mut text)));
                    }
                    out.push(Token::Unclosed);
                    text.push_str(r);
                    rest = "";
                }
            }
        } else {
            let c = rest.chars().next().unwrap();
            text.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if !text.is_empty() {
        out.push(Token::Text(text));
    }
    out
}

// ----- matching what was typed -----------------------------------------------

/// The triggers of the snippets that expand as you type, longest first, so
/// `;sig2` wins over `;sig` when both end what was typed.
#[derive(Clone, Debug, Default)]
pub struct Triggers {
    list: Vec<(Vec<char>, String)>,
    longest: usize,
}

impl Triggers {
    pub fn new(snippets: &[Snippet]) -> Self {
        let mut list: Vec<(Vec<char>, String)> = snippets
            .iter()
            .filter(|s| s.auto_expand && !s.trigger.is_empty())
            .map(|s| (s.trigger.chars().collect(), s.id.clone()))
            .collect();
        list.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then(a.0.cmp(&b.0)));
        let longest = list.first().map_or(0, |(t, _)| t.len());
        Triggers { list, longest }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// What was typed since the last reset (click, focus change, navigation key,
/// Ctrl/Alt chord), as far back as the longest trigger needs.
#[derive(Clone, Debug, Default)]
pub struct Typed {
    chars: Vec<char>,
    /// Whether `chars` starts where typing started after a reset (that
    /// counts as a word boundary) or the oldest characters were dropped.
    from_reset: bool,
}

/// A trigger found at the end of what was typed.
#[derive(Debug, PartialEq, Eq)]
pub struct Found {
    pub snippet_id: String,
    /// Characters to erase before typing the expansion.
    pub erase: usize,
}

impl Typed {
    pub fn new() -> Self {
        Typed {
            chars: Vec::new(),
            from_reset: true,
        }
    }

    pub fn reset(&mut self) {
        self.chars.clear();
        self.from_reset = true;
    }

    pub fn backspace(&mut self) {
        if self.chars.pop().is_none() {
            // Erasing text from before the reset: we no longer know what precedes.
            self.from_reset = false;
        }
    }

    /// Adds a typed character and returns the snippet whose trigger it
    /// completes, if any.
    ///
    /// A trigger that starts with a letter or digit must start a word: the
    /// character before it isn't a letter or digit (`sig` doesn't fire in
    /// "design"). Triggers starting with a symbol (`;sig`, `//date`) fire
    /// anywhere.
    pub fn push(&mut self, c: char, triggers: &Triggers) -> Option<Found> {
        self.chars.push(c);
        let keep = (triggers.longest + 1).max(1);
        if self.chars.len() > keep {
            let drop = self.chars.len() - keep;
            self.chars.drain(..drop);
            self.from_reset = false;
        }
        for (t, id) in &triggers.list {
            if t.len() > self.chars.len() || !self.chars.ends_with(t) {
                continue;
            }
            let start = self.chars.len() - t.len();
            let boundary = if t[0].is_alphanumeric() {
                match start.checked_sub(1).map(|i| self.chars[i]) {
                    Some(prev) => !prev.is_alphanumeric(),
                    None => self.from_reset,
                }
            } else {
                true
            };
            if boundary {
                let found = Found {
                    snippet_id: id.clone(),
                    erase: t.len(),
                };
                self.reset();
                return Some(found);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn cx() -> Context {
        Context {
            now: Local.with_ymd_and_hms(2026, 11, 19, 14, 5, 0).unwrap(),
            clipboard: Some("CLIP".into()),
        }
    }

    #[test]
    fn placeholders() {
        assert_eq!(
            render("Hi {{clipboard}}, it's {{date}} {{time}}", &cx()).text,
            "Hi CLIP, it's 2026-11-19 14:05"
        );
        assert_eq!(render("{{date:%d/%m/%Y}}", &cx()).text, "19/11/2026");
        assert_eq!(render("{{datetime}}", &cx()).text, "2026-11-19 14:05");
        assert_eq!(render("a \\{{x}} b", &cx()).text, "a {{x}} b");
        let r = render("<b>{{cursor}}</b>", &cx());
        assert_eq!((r.text.as_str(), r.cursor_back), ("<b></b>", 4));
        assert_eq!(render("plain", &cx()).cursor_back, 0);
    }

    #[test]
    fn checking() {
        assert!(check("ok {{date}} {{cursor}}").is_ok());
        assert!(
            check("{{nope}}")
                .unwrap_err()
                .contains("isn't a placeholder")
        );
        assert!(
            check("{{cursor}}{{cursor}}")
                .unwrap_err()
                .contains("only once")
        );
        assert!(check("{{date").unwrap_err().contains("without"));
        assert!(check("{{date:%Q}}").is_err());
    }

    fn snippet(trigger: &str, id: &str) -> Snippet {
        Snippet {
            id: id.into(),
            name: id.into(),
            trigger: trigger.into(),
            text: "x".into(),
            auto_expand: true,
        }
    }

    fn typing(triggers: &Triggers, s: &str) -> Vec<Found> {
        let mut t = Typed::new();
        s.chars().filter_map(|c| t.push(c, triggers)).collect()
    }

    #[test]
    fn longest_trigger_wins_and_words_matter() {
        // (A trigger that starts another one is refused by config validation:
        // it would always fire first.)
        let tr = Triggers::new(&[snippet(";sig", "short"), snippet("brb", "brb")]);
        assert_eq!(
            typing(&tr, "x;sig"),
            [Found {
                snippet_id: "short".into(),
                erase: 4
            }]
        );
        assert_eq!(
            typing(&tr, "ok brb"),
            [Found {
                snippet_id: "brb".into(),
                erase: 3
            }]
        );
        assert!(typing(&tr, "abrb").is_empty(), "inside a word");
        assert_eq!(typing(&tr, "brb").len(), 1, "a reset counts as a boundary");
        let tr = Triggers::new(&[snippet(";a", "a"), snippet("x;a", "xa")]);
        assert_eq!(
            typing(&tr, " x;a")[0].snippet_id,
            "xa",
            "the longer trigger wins"
        );
        assert_eq!(
            typing(&tr, "zx;a")[0].snippet_id,
            "a",
            "x;a would start inside a word"
        );
    }

    #[test]
    fn reset_and_backspace() {
        let tr = Triggers::new(&[snippet(";sig", "s")]);
        let mut t = Typed::new();
        for c in ";si".chars() {
            assert!(t.push(c, &tr).is_none());
        }
        t.reset();
        assert!(
            t.push('g', &tr).is_none(),
            "a click in between breaks the trigger"
        );
        let mut t = Typed::new();
        for c in ";sx".chars() {
            t.push(c, &tr);
        }
        t.backspace();
        t.push('i', &tr);
        assert!(t.push('g', &tr).is_some(), "a fixed typo still expands");
    }

    #[test]
    fn only_snippets_that_expand_as_you_type() {
        let mut s = snippet(";x", "x");
        s.auto_expand = false;
        assert!(Triggers::new(&[s]).is_empty());
    }
}
