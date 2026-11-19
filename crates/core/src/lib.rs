//! RedSnap's core: query matching and ranking, the calculator, providers,
//! config and the usage store. No OS or UI code, so all of it is tested on
//! any platform.

pub mod calc;
pub mod item;
pub mod matcher;
pub mod text;
