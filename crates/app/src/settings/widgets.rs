//! Shortcuts as `global-hotkey` specs (`Ctrl+Alt+Space`): from a key press in
//! the recorder, and for display.

use gpui::Keystroke;

/// A key press as a `global-hotkey` spec: `Ctrl+Alt+Space`, `Alt+Super+C`.
/// `None` for a modifier alone, a key `global-hotkey` doesn't name here, or
/// a plain key (it would fire while typing anywhere; F-keys may stand alone).
pub fn spec_from_keystroke(k: &Keystroke) -> Option<String> {
    let key = k.key.as_str();
    let named = match key {
        "space" => "Space",
        "enter" => "Enter",
        "tab" => "Tab",
        "`" => "Backquote",
        "-" => "Minus",
        "=" => "Equal",
        "," => "Comma",
        "." => "Period",
        "/" => "Slash",
        ";" => "Semicolon",
        "'" => "Quote",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "\\" => "Backslash",
        "up" => "ArrowUp",
        "down" => "ArrowDown",
        "left" => "ArrowLeft",
        "right" => "ArrowRight",
        _ => "",
    };
    let function_key = key.strip_prefix('f').filter(|n| !n.is_empty() && n.len() <= 2 && n.bytes().all(|b| b.is_ascii_digit()));
    let key = if !named.is_empty() {
        named.to_string()
    } else if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() {
        key.to_ascii_uppercase()
    } else {
        format!("F{}", function_key?)
    };
    let m = &k.modifiers;
    let mods: Vec<&str> = [(m.control, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.platform, "Super")]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
    if mods.is_empty() && !key.starts_with('F') {
        return None;
    }
    if mods.is_empty() && key.len() == 1 {
        // A lone "F" key press, not a function key.
        return None;
    }
    Some(mods.into_iter().chain([key.as_str()]).collect::<Vec<_>>().join("+"))
}

/// How a spec reads: `Ctrl + Alt + Space`, or `⌥⌘C` on macOS.
pub fn display_shortcut(spec: &str) -> String {
    let parts = spec.split('+');
    if cfg!(target_os = "macos") {
        parts
            .map(|p| match p {
                "Ctrl" => "⌃",
                "Alt" => "⌥",
                "Shift" => "⇧",
                "Super" | "Cmd" => "⌘",
                other => other,
            })
            .collect()
    } else {
        parts.map(|p| if p == "Super" { "Win" } else { p }).collect::<Vec<_>>().join(" + ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &'static str) -> Option<String> {
        spec_from_keystroke(&Keystroke::parse(s).unwrap())
    }

    #[test]
    fn key_presses_to_specs() {
        assert_eq!(spec("alt-space").as_deref(), Some("Alt+Space"));
        assert_eq!(spec("ctrl-alt-c").as_deref(), Some("Ctrl+Alt+C"));
        assert_eq!(spec("ctrl-shift-1").as_deref(), Some("Ctrl+Shift+1"));
        assert_eq!(spec("super-alt-up").as_deref(), Some("Alt+Super+ArrowUp"));
        assert_eq!(spec("ctrl-`").as_deref(), Some("Ctrl+Backquote"));
        assert_eq!(spec("f5").as_deref(), Some("F5"));
        assert_eq!(spec("ctrl-f12").as_deref(), Some("Ctrl+F12"));
        // Plain keys would fire while typing.
        assert_eq!(spec("a"), None);
        assert_eq!(spec("f"), None);
        assert_eq!(spec("space"), None);
    }

    #[test]
    fn shown() {
        if !cfg!(target_os = "macos") {
            assert_eq!(display_shortcut("Ctrl+Alt+Space"), "Ctrl + Alt + Space");
            assert_eq!(display_shortcut("Alt+Super+C"), "Alt + Win + C");
        }
    }
}
