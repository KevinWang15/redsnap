//! Matching a query against an item's names, with a match class that ranks
//! before any score: a literal match never loses to scattered fuzzy letters.

use std::cmp::Ordering;

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32String};

use crate::text;

/// How well a query matched, best first (derives `Ord` in this order).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchClass {
    /// The whole name (or alias / pinyin spelling) equals the query.
    Exact,
    /// The name starts with the query.
    Prefix,
    /// A word in the name starts with the query, or the name's initials do
    /// (`vsc` → Visual Studio Code, `wx` → 微信).
    WordPrefix,
    /// The query appears inside the name.
    Substring,
    /// The query's characters appear in order, not together.
    Fuzzy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    pub class: MatchClass,
    /// Within a class, higher is better (nucleo's score, plus a bonus for
    /// shorter names).
    pub score: u32,
}

impl Match {
    /// Ordering for ranking: better first.
    pub fn better(&self, other: &Match) -> Ordering {
        self.class.cmp(&other.class).then(other.score.cmp(&self.score))
    }
}

/// One spelling of a name, prepared once when the catalog is built.
#[derive(Clone, Debug)]
pub struct Haystack {
    folded: String,
    word_starts: Vec<usize>,
    acronym: String,
    utf32: Utf32String,
    /// Pinyin and acronym spellings never count as `Exact`.
    exact_allowed: bool,
}

impl Haystack {
    fn new(original: &str, exact_allowed: bool) -> Self {
        let folded = text::fold(original);
        let word_starts = text::word_starts(original, &folded);
        let acronym = text::acronym(original, &folded);
        let utf32 = Utf32String::from(folded.as_str());
        Haystack { folded, word_starts, acronym, utf32, exact_allowed }
    }
}

/// All the spellings an item can be found by: its name, aliases, and pinyin.
#[derive(Clone, Debug, Default)]
pub struct Names {
    hay: Vec<Haystack>,
}

impl Names {
    /// `name` is the display name; `aliases` are extra names (executable
    /// name, bundle name, user aliases, keywords).
    pub fn new<'a>(name: &'a str, aliases: impl IntoIterator<Item = &'a str>) -> Self {
        let mut hay = Vec::new();
        let mut add = |s: &str, exact: bool| {
            if !s.trim().is_empty() {
                hay.push(Haystack::new(s, exact));
            }
        };
        for (i, n) in std::iter::once(name).chain(aliases).enumerate() {
            add(n, true);
            // Pinyin for the name and aliases alike, but only the first few
            // spellings of aliases.
            for (full, initials) in text::pinyin_forms(n).into_iter().take(if i == 0 { 16 } else { 4 }) {
                add(&full, false);
                add(&initials, false);
            }
        }
        Names { hay }
    }
}

/// A query prepared once per keystroke.
pub struct PreparedQuery {
    folded: String,
    pattern: Pattern,
}

impl PreparedQuery {
    pub fn new(query: &str) -> Self {
        let folded = text::fold(query.trim());
        let pattern = Pattern::new(&folded, CaseMatching::Ignore, Normalization::Never, AtomKind::Fuzzy);
        PreparedQuery { folded, pattern }
    }

    pub fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }

    pub fn folded(&self) -> &str {
        &self.folded
    }

    /// The best match over all of `names`' spellings, or `None`.
    pub fn best(&self, names: &Names, matcher: &mut Matcher) -> Option<Match> {
        names.hay.iter().filter_map(|h| self.one(h, matcher)).min_by(|a, b| a.better(b))
    }

    fn one(&self, h: &Haystack, matcher: &mut Matcher) -> Option<Match> {
        let q = self.folded.as_str();
        if q.is_empty() {
            return None;
        }
        // Shorter names first within a class: "Code" before "Code Insiders".
        let short = 1000u32.saturating_sub(h.folded.chars().count() as u32);
        let literal = |class| Some(Match { class, score: 10_000 + short });
        if h.exact_allowed && h.folded == q {
            return literal(MatchClass::Exact);
        }
        if h.folded.starts_with(q) {
            return literal(MatchClass::Prefix);
        }
        let q_compact: String = q.chars().filter(|c| !c.is_whitespace()).collect();
        if h.word_starts.iter().any(|&i| h.folded[i..].starts_with(q))
            || (q_compact.chars().count() >= 2 && h.acronym.starts_with(&q_compact))
        {
            return literal(MatchClass::WordPrefix);
        }
        if h.folded.contains(q) {
            return literal(MatchClass::Substring);
        }
        let score = self.pattern.score(h.utf32.slice(..), matcher)?;
        Some(Match { class: MatchClass::Fuzzy, score: score + short / 10 })
    }
}

/// The matcher scratch space; one per thread.
pub fn new_matcher() -> Matcher {
    Matcher::new(Config::DEFAULT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(query: &str, name: &str) -> Option<MatchClass> {
        PreparedQuery::new(query).best(&Names::new(name, []), &mut new_matcher()).map(|m| m.class)
    }

    #[test]
    fn classes() {
        use MatchClass::*;
        assert_eq!(m("safari", "Safari"), Some(Exact));
        assert_eq!(m("saf", "Safari"), Some(Prefix));
        assert_eq!(m("studio", "Visual Studio Code"), Some(WordPrefix));
        assert_eq!(m("vsc", "Visual Studio Code"), Some(WordPrefix));
        assert_eq!(m("udio", "Visual Studio Code"), Some(Substring));
        assert_eq!(m("vscd", "Visual Studio Code"), Some(Fuzzy));
        assert_eq!(m("xyz", "Visual Studio Code"), None);
    }

    #[test]
    fn pinyin_finds_chinese_names() {
        use MatchClass::*;
        assert_eq!(m("wx", "微信"), Some(Prefix));
        assert_eq!(m("weixin", "微信"), Some(Prefix));
        assert_eq!(m("微信", "微信"), Some(Exact));
        assert_eq!(m("yinyue", "网易云音乐"), Some(Substring));
    }

    #[test]
    fn accents_and_case_are_ignored() {
        assert_eq!(m("cafe", "Café"), Some(MatchClass::Exact));
    }

    #[test]
    fn literal_beats_fuzzy_whatever_the_score() {
        let q = PreparedQuery::new("code");
        let mut mm = new_matcher();
        let literal = q.best(&Names::new("Visual Studio Code", []), &mut mm).unwrap();
        let fuzzy = q.best(&Names::new("Color Desk", []), &mut mm).unwrap();
        assert_eq!(fuzzy.class, MatchClass::Fuzzy);
        assert_eq!(literal.better(&fuzzy), Ordering::Less);
    }

    #[test]
    fn aliases_match_too() {
        let names = Names::new("Visual Studio Code", ["code.exe"]);
        let got = PreparedQuery::new("code.exe").best(&names, &mut new_matcher()).unwrap();
        assert_eq!(got.class, MatchClass::Exact);
    }
}
