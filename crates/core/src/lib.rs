//! RedSnap's core: query matching and ranking, the calculator, providers,
//! config and the usage store. No OS or UI code, so all of it is tested on
//! any platform.

pub mod calc;
pub mod clipboard;
pub mod config;
pub mod item;
pub mod matcher;
pub mod providers;
pub mod query;
pub mod search;
pub mod snippets;
pub mod text;
pub mod usage;
