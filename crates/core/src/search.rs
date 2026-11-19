//! The search pipeline: run every provider over in-memory data, rank, keep
//! the best few.
//!
//! Everything here runs on the UI thread per keystroke, so providers do no IO
//! and no locking; usage data is one snapshot per search.

use std::cmp::Reverse;
use std::sync::Arc;


use nucleo_matcher::Matcher;

use crate::item::{Item, ProviderId};
use crate::matcher::{Match, MatchClass, new_matcher};
use crate::query::{Keywords, Query};
use crate::usage::Usage;

/// Where a candidate goes regardless of score.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Slot {
    /// Above everything: a calculator result, a typed URL or path, a keyword
    /// route's result.
    Top,
    Normal,
    /// Shown only when nothing else matched ("Search the web for …").
    Fallback,
}

#[derive(Clone, Debug)]
pub struct Candidate {
    /// Shared: providers keep prebuilt items, so matching thousands of them
    /// copies no strings.
    pub item: Arc<Item>,
    pub m: Match,
    pub slot: Slot,
}

impl Candidate {
    pub fn normal(item: Arc<Item>, m: Match) -> Self {
        Candidate { item, m, slot: Slot::Normal }
    }

    pub fn top(item: Item) -> Self {
        Candidate { item: Arc::new(item), m: Match { class: MatchClass::Exact, score: u32::MAX }, slot: Slot::Top }
    }

    pub fn fallback(item: Item) -> Self {
        Candidate { item: Arc::new(item), m: Match { class: MatchClass::Fuzzy, score: 0 }, slot: Slot::Fallback }
    }
}

/// Per-search scratch the providers share.
pub struct SearchContext {
    pub matcher: Matcher,
    pub now: i64,
}

pub trait Provider {
    fn id(&self) -> ProviderId;

    /// Appends candidates for `q`. With an empty query, appends the items
    /// that may appear in the default list (ranked by use), if any. Must not
    /// do IO.
    fn search(&self, q: &Query, cx: &mut SearchContext, out: &mut Vec<Candidate>);
}

/// A ranked result.
#[derive(Clone, Debug)]
pub struct Hit {
    pub item: Arc<Item>,
    pub class: MatchClass,
}

pub struct Engine {
    providers: Vec<Box<dyn Provider>>,
    keywords: Keywords,
    cx: SearchContext,
}

impl Engine {
    pub fn new(providers: Vec<Box<dyn Provider>>, keywords: Keywords) -> Self {
        Engine { providers, keywords, cx: SearchContext { matcher: new_matcher(), now: 0 } }
    }

    pub fn keywords(&self) -> &Keywords {
        &self.keywords
    }

    /// The best `limit` results for `input`, best first.
    pub fn search(&mut self, input: &str, usage: &Arc<Usage>, now: i64, limit: usize) -> (Query, Vec<Hit>) {
        let q = Query::parse(input, &self.keywords);
        self.cx.now = now;
        let mut all = Vec::new();
        for p in &self.providers {
            p.search(&q, &mut self.cx, &mut all);
        }
        let hits = rank(all, &q, usage, now, limit);
        (q, hits)
    }
}

/// Sorts candidates and keeps the best `limit`:
/// 1. slot (top results, then normal ones, then fallbacks, and fallbacks only
///    when nothing else is there);
/// 2. literal before fuzzy: a match of the typed text never loses to
///    scattered letters, whatever was learned;
/// 3. how often this item was picked for this query (so picking Chrome for
///    "c" puts it above other literal matches of "c");
/// 4. match class (exact, prefix, word, substring);
/// 5. match score plus a small bonus for overall use;
/// 6. provider, then id, for a stable order.
pub fn rank(mut all: Vec<Candidate>, q: &Query, usage: &Usage, now: i64, limit: usize) -> Vec<Hit> {
    if all.iter().any(|c| c.slot != Slot::Fallback) {
        all.retain(|c| c.slot != Slot::Fallback);
    }
    let folded = q.prepared.folded();
    let mut keyed: Vec<(Key, Candidate)> = all
        .into_iter()
        .map(|c| {
            let learn = c.item.learnable;
            let boost = if learn { usage.query_boost(folded, &c.item.id, now) } else { 0 };
            let global = if learn { usage.global(&c.item.id, now) } else { 0 };
            let key = Key {
                slot: c.slot,
                fuzzy: c.m.class == MatchClass::Fuzzy,
                boost: Reverse(boost),
                class: c.m.class,
                score: Reverse(c.m.score.saturating_add(global)),
                provider: c.item.provider,
            };
            (key, c)
        })
        .collect();
    let cmp = |a: &(Key, Candidate), b: &(Key, Candidate)| a.0.cmp(&b.0).then_with(|| a.1.item.id.cmp(&b.1.item.id));
    if limit < keyed.len() {
        if limit == 0 {
            return Vec::new();
        }
        keyed.select_nth_unstable_by(limit - 1, cmp);
        keyed.truncate(limit);
    }
    keyed.sort_unstable_by(cmp);
    // The same item from two providers (an app found by name and by alias, say):
    // keep the better one.
    let mut seen = std::collections::HashSet::new();
    keyed
        .into_iter()
        .filter(|(_, c)| seen.insert(c.item.id.clone()))
        .map(|(_, c)| Hit { class: c.m.class, item: c.item })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    slot: Slot,
    fuzzy: bool,
    boost: Reverse<u32>,
    class: MatchClass,
    score: Reverse<u32>,
    provider: ProviderId,
}
