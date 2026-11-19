//! Turning the typed text into a query: the prepared matcher input and the
//! keyword route (`g rust` searches Google for "rust").

use crate::matcher::PreparedQuery;
use crate::text;

/// What a keyword at the start of the query targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    WebSearch(String),
    QuickLink(String),
    /// `sn …`: snippets only.
    Snippets,
    /// `f …`: the OS's file index.
    Files,
}

/// Keywords from the config, looked up by their folded form.
#[derive(Clone, Debug, Default)]
pub struct Keywords {
    entries: Vec<(String, Target)>,
}

impl Keywords {
    pub fn new(entries: impl IntoIterator<Item = (String, Target)>) -> Self {
        Keywords {
            entries: entries
                .into_iter()
                .filter(|(k, _)| !k.is_empty())
                .map(|(k, t)| (text::fold(&k), t))
                .collect(),
        }
    }

    fn find(&self, folded: &str) -> Option<&Target> {
        self.entries
            .iter()
            .find(|(k, _)| k == folded)
            .map(|(_, t)| t)
    }
}

pub struct Query {
    /// As typed.
    pub raw: String,
    /// Trimmed.
    pub text: String,
    pub prepared: PreparedQuery,
    /// Set when the query starts with a keyword followed by a space: the
    /// target and the rest of the text.
    pub route: Option<(Target, String)>,
}

impl Query {
    pub fn parse(raw: &str, keywords: &Keywords) -> Self {
        let text = raw.trim().to_string();
        let route = raw
            .trim_start()
            .split_once(char::is_whitespace)
            .and_then(|(first, rest)| {
                keywords
                    .find(&text::fold(first))
                    .map(|t| (t.clone(), rest.trim().to_string()))
            });
        Query {
            raw: raw.to_string(),
            prepared: PreparedQuery::new(&text),
            text,
            route,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keywords() -> Keywords {
        Keywords::new([("g".to_string(), Target::WebSearch("google".into()))])
    }

    #[test]
    fn keyword_needs_a_space_after_it() {
        let q = Query::parse("g rust gpui ", &keywords());
        assert_eq!(
            q.route,
            Some((Target::WebSearch("google".into()), "rust gpui".into()))
        );
        assert_eq!(Query::parse("g", &keywords()).route, None);
        assert_eq!(Query::parse("google", &keywords()).route, None);
        assert_eq!(
            Query::parse("G x", &keywords()).route.map(|r| r.1),
            Some("x".into())
        );
    }

    #[test]
    fn keyword_with_nothing_after_routes_with_empty_rest() {
        let q = Query::parse("g ", &keywords());
        assert_eq!(q.route.map(|r| r.1), Some(String::new()));
    }
}
