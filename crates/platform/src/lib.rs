//! Every OS call RedSnap makes, per OS behind the same functions. The rest of
//! the app never uses `cfg(target_os)` for behavior.

pub mod apps;
pub mod autostart;
pub mod clipboard;
pub mod icon;
pub mod instance;
pub mod open;
pub mod paths;
pub mod settings_ipc;
pub mod tray;
pub mod window;
