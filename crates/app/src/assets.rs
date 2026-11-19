//! Assets compiled into the binary: the UI icons (Lucide, ISC license; see
//! assets/icons/LICENSE-lucide). gpui draws SVGs tinted with the text color.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

/// Icon paths for `gpui::svg().path(..)`.
pub mod icon {
    pub const ARROW_RIGHT_TO_LINE: &str = "icons/arrow-right-to-line.svg";
    pub const ARROW_UP_DOWN: &str = "icons/arrow-up-down.svg";
    pub const BAN: &str = "icons/ban.svg";
    pub const CALCULATOR: &str = "icons/calculator.svg";
    pub const CORNER_DOWN_LEFT: &str = "icons/corner-down-left.svg";
    pub const EXTERNAL_LINK: &str = "icons/external-link.svg";
    pub const FILES: &str = "icons/files.svg";
    pub const FOLDER: &str = "icons/folder.svg";
    pub const IMAGE: &str = "icons/image.svg";
    pub const INFO: &str = "icons/info.svg";
    pub const MARK_BRACKETS: &str = "icons/mark-brackets.svg";
    pub const MARK_SQUARE: &str = "icons/mark-square.svg";
    pub const PIN: &str = "icons/pin.svg";
    pub const POWER: &str = "icons/power.svg";
    pub const REFRESH_CW: &str = "icons/refresh-cw.svg";
    pub const SEARCH: &str = "icons/search.svg";
    pub const SETTINGS: &str = "icons/settings.svg";
    pub const SQUARE_PEN: &str = "icons/square-pen.svg";
    pub const TYPE: &str = "icons/type.svg";
    pub const X: &str = "icons/x.svg";
}

const FILES: &[(&str, &[u8])] = &[
    ("icons/arrow-right-to-line.svg", include_bytes!("../assets/icons/arrow-right-to-line.svg")),
    ("icons/arrow-up-down.svg", include_bytes!("../assets/icons/arrow-up-down.svg")),
    ("icons/ban.svg", include_bytes!("../assets/icons/ban.svg")),
    ("icons/calculator.svg", include_bytes!("../assets/icons/calculator.svg")),
    ("icons/corner-down-left.svg", include_bytes!("../assets/icons/corner-down-left.svg")),
    ("icons/external-link.svg", include_bytes!("../assets/icons/external-link.svg")),
    ("icons/files.svg", include_bytes!("../assets/icons/files.svg")),
    ("icons/folder.svg", include_bytes!("../assets/icons/folder.svg")),
    ("icons/image.svg", include_bytes!("../assets/icons/image.svg")),
    ("icons/info.svg", include_bytes!("../assets/icons/info.svg")),
    ("icons/mark-brackets.svg", include_bytes!("../assets/icons/mark-brackets.svg")),
    ("icons/mark-square.svg", include_bytes!("../assets/icons/mark-square.svg")),
    ("icons/pin.svg", include_bytes!("../assets/icons/pin.svg")),
    ("icons/power.svg", include_bytes!("../assets/icons/power.svg")),
    ("icons/refresh-cw.svg", include_bytes!("../assets/icons/refresh-cw.svg")),
    ("icons/search.svg", include_bytes!("../assets/icons/search.svg")),
    ("icons/settings.svg", include_bytes!("../assets/icons/settings.svg")),
    ("icons/square-pen.svg", include_bytes!("../assets/icons/square-pen.svg")),
    ("icons/type.svg", include_bytes!("../assets/icons/type.svg")),
    ("icons/x.svg", include_bytes!("../assets/icons/x.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES.iter().filter(|(p, _)| p.starts_with(path)).map(|(p, _)| SharedString::from(*p)).collect())
    }
}
