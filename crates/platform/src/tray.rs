//! The tray / menu bar icon and its menu. `tray-icon` on Windows and macOS
//! (created on the main thread, whose message loop gpui runs); a
//! StatusNotifierItem over D-Bus on Linux (`ksni`, on its own thread, no GTK).

use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    Settings,
    ToggleStartAtLogin,
    Quit,
}

pub type OnCommand = Box<dyn Fn(TrayCommand) + Send + Sync + 'static>;

const LOGIN_LABEL: &str = if cfg!(windows) { "Start with Windows" } else { "Start at login" };

pub use imp::Tray;

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod imp {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

    pub struct Tray {
        _icon: TrayIcon,
        login: CheckMenuItem,
    }

    impl Tray {
        /// Call on the main thread.
        pub fn new(start_at_login: bool, on_command: OnCommand) -> Result<Self> {
            let on_command = std::sync::Arc::new(on_command);
            let open = MenuItem::with_id("open", "Open RedSnap", true, None);
            let settings = MenuItem::with_id("settings", "Settings…", true, None);
            let login = CheckMenuItem::with_id("login", LOGIN_LABEL, true, start_at_login, None);
            let quit = MenuItem::with_id("quit", "Quit RedSnap", true, None);
            let menu = Menu::new();
            menu.append_items(&[&open, &settings, &login, &PredefinedMenuItem::separator(), &quit])?;
            let menu_cb = on_command.clone();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                let cmd = match event.id.0.as_str() {
                    "open" => TrayCommand::Open,
                    "settings" => TrayCommand::Settings,
                    "login" => TrayCommand::ToggleStartAtLogin,
                    "quit" => TrayCommand::Quit,
                    _ => return,
                };
                menu_cb(cmd);
            }));
            // Windows convention: left click opens RedSnap, right click the menu.
            TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
                if cfg!(windows)
                    && let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event
                {
                    on_command(TrayCommand::Open);
                }
            }));
            let size = 32;
            // The app icon (mark on a dark tile): the bare white brackets vanish
            // on a light taskbar. macOS menu bars use the bare mark.
            let rgba = if cfg!(target_os = "macos") { crate::icon::logo_rgba(size) } else { crate::icon::app_icon_rgba(size) };
            let icon = Icon::from_rgba(rgba, size, size)?;
            let builder = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(cfg!(target_os = "macos"))
                .with_tooltip("RedSnap")
                .with_icon(icon);
            Ok(Tray { _icon: builder.build()?, login })
        }

        pub fn set_start_at_login(&self, on: bool) {
            self.login.set_checked(on);
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;
    use ksni::blocking::TrayMethods;
    use ksni::menu::{CheckmarkItem, StandardItem};

    struct Sni {
        login: bool,
        on_command: OnCommand,
    }

    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "redsnap".into()
        }

        fn title(&self) -> String {
            "RedSnap".into()
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let size = 32;
            // ARGB32, network byte order.
            let data = crate::icon::app_icon_rgba(size).as_chunks::<4>().0.iter().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
            vec![ksni::Icon { width: size as i32, height: size as i32, data }]
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            (self.on_command)(TrayCommand::Open);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let item = |label: &str, cmd: TrayCommand| {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |t: &mut Sni| (t.on_command)(cmd)),
                    ..Default::default()
                }
                .into()
            };
            vec![
                item("Open RedSnap", TrayCommand::Open),
                item("Settings…", TrayCommand::Settings),
                CheckmarkItem {
                    label: LOGIN_LABEL.into(),
                    checked: self.login,
                    activate: Box::new(|t: &mut Sni| (t.on_command)(TrayCommand::ToggleStartAtLogin)),
                    ..Default::default()
                }
                .into(),
                ksni::MenuItem::Separator,
                item("Quit RedSnap", TrayCommand::Quit),
            ]
        }
    }

    pub struct Tray {
        handle: Option<ksni::blocking::Handle<Sni>>,
    }

    impl Tray {
        /// Without a StatusNotifier host (plain X11 window managers, GNOME
        /// without the AppIndicator extension) there's no tray; RedSnap still
        /// works through its hotkey and `redsnap toggle`.
        pub fn new(start_at_login: bool, on_command: OnCommand) -> Result<Self> {
            let handle = Sni { login: start_at_login, on_command }
                .spawn()
                .map_err(|e| log::info!("no tray on this desktop: {e}"))
                .ok();
            Ok(Tray { handle })
        }

        pub fn set_start_at_login(&self, on: bool) {
            if let Some(h) = &self.handle {
                h.update(|t| t.login = on);
            }
        }
    }
}
