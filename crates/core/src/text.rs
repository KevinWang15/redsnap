//! Text normalization shared by matching and search: one fold applied to
//! both the query and the text it is matched against.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

/// Lowercase, with accents removed (NFKD, combining marks dropped), so
/// `Café` and `cafe` compare equal. CJK characters are unchanged.
pub fn fold(s: &str) -> String {
    s.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Whether `c` is a CJK ideograph (the characters pinyin applies to).
pub fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2EBEF)
}

/// Most pinyin spellings kept for one name. Characters with several readings
/// (乐: le / yue) multiply the spellings; past this many, further readings
/// are dropped.
const MAX_PINYIN_FORMS: usize = 16;

/// Pinyin spellings of a name that contains Chinese characters: the full
/// pinyin without tones (`微信` → `weixin`) and the initials (`wx`), one pair
/// per combination of readings (`音乐` → `yinle`/`yl` and `yinyue`/`yy`).
/// Non-Chinese characters are kept (folded). Empty if there is no Chinese
/// character in `name`.
pub fn pinyin_forms(name: &str) -> Vec<(String, String)> {
    use pinyin::ToPinyinMulti;
    if !name.chars().any(is_han) {
        return Vec::new();
    }
    let mut forms = vec![(String::new(), String::new())];
    for c in name.chars() {
        let readings: Vec<(&str, &str)> = match c.to_pinyin_multi() {
            Some(multi) => {
                let mut r: Vec<(&str, &str)> = Vec::new();
                for p in multi {
                    if !r.iter().any(|(plain, _)| *plain == p.plain()) {
                        r.push((p.plain(), p.first_letter()));
                    }
                }
                r
            }
            None => {
                if c.is_alphanumeric() {
                    let f = fold(&c.to_string());
                    for (full, initials) in &mut forms {
                        full.push_str(&f);
                        initials.push_str(&f);
                    }
                }
                continue;
            }
        };
        let mut next = Vec::new();
        for (full, initials) in &forms {
            for (i, (plain, first)) in readings.iter().enumerate() {
                if i > 0 && next.len() >= MAX_PINYIN_FORMS {
                    break;
                }
                next.push((format!("{full}{plain}"), format!("{initials}{first}")));
            }
        }
        next.truncate(MAX_PINYIN_FORMS);
        forms = next;
    }
    forms
}

/// The start offsets (in bytes) of the words in a folded string: after a
/// non-alphanumeric character, and at lowercase→uppercase steps in the
/// original (`VSCode` → `VS`, `Code`). `original` and `folded` must be the same
/// text before and after [`fold`]; camel-case steps are found in `original`
/// only when folding didn't change its length.
pub fn word_starts(original: &str, folded: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut prev: Option<char> = None;
    for (i, c) in folded.char_indices() {
        let boundary = match prev {
            None => true,
            Some(p) => !p.is_alphanumeric() && c.is_alphanumeric(),
        };
        if boundary && c.is_alphanumeric() {
            starts.push(i);
        }
        prev = Some(c);
    }
    if original.len() == folded.len() {
        let chars: Vec<(usize, char)> = original.char_indices().collect();
        for (k, &(i, b)) in chars.iter().enumerate().skip(1) {
            let a = chars[k - 1].1;
            let after = chars.get(k + 1).map(|&(_, c)| c);
            // `aB` (camelCase) or `ABc` (the C of `VSCode`).
            let step = (a.is_lowercase() && b.is_uppercase())
                || (a.is_uppercase() && b.is_uppercase() && after.is_some_and(char::is_lowercase));
            if step && !starts.contains(&i) {
                starts.push(i);
            }
        }
        starts.sort_unstable();
    }
    starts
}

/// The first letter of every word: `Visual Studio Code` → `vsc`.
pub fn acronym(original: &str, folded: &str) -> String {
    word_starts(original, folded)
        .iter()
        .filter_map(|&i| folded[i..].chars().next())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fold_lowercases_and_drops_accents() {
        assert_eq!(fold("Café Ünïcode"), "cafe unicode");
        assert_eq!(fold("微信"), "微信");
    }

    #[test]
    fn pinyin_full_and_initials() {
        let has = |name: &str, full: &str, initials: &str| {
            pinyin_forms(name).contains(&(full.to_string(), initials.to_string()))
        };
        assert!(has("微信", "weixin", "wx"));
        assert!(has("网易云音乐", "wangyiyunyinyue", "wyyyy"));
        assert!(has("网易云音乐", "wangyiyunyinle", "wyyyl"));
        assert!(has("QQ音乐", "qqyinyue", "qqyy"));
        assert!(pinyin_forms("Safari").is_empty());
        assert!(pinyin_forms("长长长长长长长长").len() <= MAX_PINYIN_FORMS);
    }

    #[test]
    fn acronyms_from_words_and_camel_case() {
        let a = |s: &str| acronym(s, &fold(s));
        assert_eq!(a("Visual Studio Code"), "vsc");
        assert_eq!(a("VSCode"), "vc");
        assert_eq!(a("GitHub Desktop"), "ghd");
        assert_eq!(a("system-settings"), "ss");
    }
}
