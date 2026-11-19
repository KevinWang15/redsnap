//! Every OS call RedSnap makes, per OS behind the same functions. The rest of
//! the app never uses `cfg(target_os)` for behavior.

pub mod autostart;
pub mod icon;
pub mod instance;
pub mod open;
pub mod paths;
pub mod tray;
pub mod window;
