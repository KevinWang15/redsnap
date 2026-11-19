//! Snippet expansion: watching what's typed in any app and replacing a
//! trigger with its snippet, and typing a snippet into the app that was in
//! front (from the launcher). Matching and templates are in
//! `redsnap_core::snippets`; this is the OS side. Windows only for now.

use std::collections::HashMap;

use redsnap_core::config;
use redsnap_core::snippets::Triggers;

use crate::clipboard::Target;

#[cfg(windows)]
mod windows;

/// What the expander needs; replaced whole when settings change.
#[derive(Clone, Default)]
pub struct ExpanderConfig {
    pub enabled: bool,
    pub triggers: Triggers,
    /// Snippet id → template.
    pub templates: HashMap<String, String>,
    /// Lowercased app names where nothing expands.
    pub excluded_apps: Vec<String>,
}

impl ExpanderConfig {
    pub fn from_config(s: &config::Snippets) -> Self {
        ExpanderConfig {
            enabled: s.expand_as_you_type,
            triggers: Triggers::new(&s.items),
            templates: s
                .items
                .iter()
                .map(|i| (i.id.clone(), i.text.clone()))
                .collect(),
            excluded_apps: s.excluded_apps.iter().map(|a| a.to_lowercase()).collect(),
        }
    }
}

/// Expands typed triggers until dropped.
pub struct Expander {
    #[cfg(windows)]
    inner: windows::Expander,
}

impl Expander {
    /// `None` where expansion as you type isn't available yet (macOS, Linux).
    pub fn start(config: ExpanderConfig) -> anyhow::Result<Option<Expander>> {
        #[cfg(windows)]
        return Ok(Some(Expander {
            inner: windows::Expander::start(config)?,
        }));
        #[cfg(not(windows))]
        {
            let _ = config;
            Ok(None)
        }
    }

    pub fn update(&self, config: ExpanderConfig) {
        #[cfg(windows)]
        self.inner.update(config);
        #[cfg(not(windows))]
        let _ = config;
    }
}

/// Types `text` into `target` (activated first), then moves the caret
/// `cursor_back` characters left. Blocks briefly; call off the UI thread.
pub fn insert(target: Target, text: &str, cursor_back: usize) -> anyhow::Result<()> {
    #[cfg(windows)]
    return windows::insert(target, text, cursor_back);
    #[cfg(not(windows))]
    {
        let _ = (target, text, cursor_back);
        anyhow::bail!("inserting snippets isn't implemented on this OS yet")
    }
}
