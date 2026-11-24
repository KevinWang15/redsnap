//! The tray / menu bar icon and its menu. `tray-icon` on Windows, a native
//! `NSStatusItem` + `NSMenu` on macOS (created on the main thread, whose
//! message loop gpui runs), and a StatusNotifierItem over D-Bus on Linux
//! (`ksni`, on its own thread, no GTK).

use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Open,
    Settings,
    ToggleStartAtLogin,
    Quit,
}

pub type OnCommand = Box<dyn Fn(TrayCommand) + Send + Sync + 'static>;

const LOGIN_LABEL: &str = if cfg!(windows) {
    "Start with Windows"
} else {
    "Start at login"
};

pub use imp::Tray;

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };

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
            menu.append_items(&[
                &open,
                &settings,
                &login,
                &PredefinedMenuItem::separator(),
                &quit,
            ])?;
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
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    on_command(TrayCommand::Open);
                }
            }));
            let size = 32;
            // The app icon (mark on a dark tile): the bare white brackets vanish
            // on a light taskbar.
            let rgba = crate::icon::app_icon_rgba(size);
            let icon = Icon::from_rgba(rgba, size, size)?;
            let builder = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip("RedSnap")
                .with_icon(icon);
            Ok(Tray {
                _icon: builder.build()?,
                login,
            })
        }

        pub fn set_start_at_login(&self, on: bool) {
            self.login.set_checked(on);
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    //! Native macOS menu bar icon (`NSStatusItem`) with a 2× Retina template
    //! image and a standard `NSMenu` attached directly to the status item so it
    //! renders cleanly on light/dark menu bars, notch displays, and menu bar
    //! managers without an overlay view.

    use anyhow::{Context as _, anyhow};
    use objc2::rc::Retained;
    use objc2::{
        AnyThread, DeclaredClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
    };
    use objc2_app_kit::{
        NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar,
        NSStatusItem, NSVariableStatusItemLength,
    };
    use objc2_foundation::{NSData, NSObject, NSSize, NSString};

    use super::*;

    const TAG_OPEN: isize = 1;
    const TAG_SETTINGS: isize = 2;
    const TAG_LOGIN: isize = 3;
    const TAG_QUIT: isize = 4;

    struct TargetIvars {
        on_command: OnCommand,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "RedSnapTrayTarget"]
        #[ivars = TargetIvars]
        struct TrayTarget;

        impl TrayTarget {
            #[unsafe(method(onMenuItem:))]
            fn on_menu_item(&self, sender: &NSMenuItem) {
                let cmd = match sender.tag() {
                    TAG_OPEN => TrayCommand::Open,
                    TAG_SETTINGS => TrayCommand::Settings,
                    TAG_LOGIN => TrayCommand::ToggleStartAtLogin,
                    TAG_QUIT => TrayCommand::Quit,
                    _ => return,
                };
                (self.ivars().on_command)(cmd);
            }
        }
    );

    impl TrayTarget {
        fn new(mtm: MainThreadMarker, on_command: OnCommand) -> Retained<Self> {
            let this = mtm.alloc().set_ivars(TargetIvars { on_command });
            unsafe { msg_send![super(this), init] }
        }
    }

    /// 36×36 px (2× Retina for an 18×18 pt menu bar icon) template image so
    /// macOS automatically tints it for light and dark menu bars.
    fn template_icon() -> Result<Retained<NSImage>> {
        let px = 36u32;
        let rgba = crate::icon::logo_rgba(px);
        let img = image::RgbaImage::from_raw(px, px, rgba)
            .ok_or_else(|| anyhow!("failed to build menu bar icon bitmap"))?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .context("encoding menu bar icon PNG")?;
        let data = NSData::from_vec(png);
        let ns_img = NSImage::initWithData(NSImage::alloc(), &data)
            .ok_or_else(|| anyhow!("NSImage failed to decode menu bar icon"))?;
        ns_img.setSize(NSSize::new(18.0, 18.0));
        ns_img.setTemplate(true);
        Ok(ns_img)
    }

    pub struct Tray {
        status_item: Retained<NSStatusItem>,
        _menu: Retained<NSMenu>,
        _target: Retained<TrayTarget>,
        login: Retained<NSMenuItem>,
    }

    impl Tray {
        /// Call on the main thread.
        pub fn new(start_at_login: bool, on_command: OnCommand) -> Result<Self> {
            let mtm = MainThreadMarker::new()
                .ok_or_else(|| anyhow!("tray must be created on the main thread"))?;
            let target = TrayTarget::new(mtm, on_command);
            let menu = NSMenu::new(mtm);
            menu.setAutoenablesItems(false);

            let make_item = |title: &str, tag: isize| -> Retained<NSMenuItem> {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(title),
                        Some(sel!(onMenuItem:)),
                        &NSString::from_str(""),
                    )
                };
                item.setTag(tag);
                item.setEnabled(true);
                unsafe { item.setTarget(Some(&target)) };
                item
            };

            let open = make_item("Open RedSnap", TAG_OPEN);
            let settings = make_item("Settings…", TAG_SETTINGS);
            let login = make_item(LOGIN_LABEL, TAG_LOGIN);
            login.setState(if start_at_login {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
            let quit = make_item("Quit RedSnap", TAG_QUIT);

            menu.addItem(&open);
            menu.addItem(&settings);
            menu.addItem(&login);
            menu.addItem(&NSMenuItem::separatorItem(mtm));
            menu.addItem(&quit);

            let status_bar = NSStatusBar::systemStatusBar();
            let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
            status_item.setAutosaveName(Some(&NSString::from_str("com.kevinwang15.redsnap.tray")));
            status_item.setVisible(true);
            let icon = template_icon()?;
            if let Some(button) = status_item.button(mtm) {
                button.setImage(Some(&icon));
                button.setToolTip(Some(&NSString::from_str("RedSnap")));
                let win_frame = button.window().map(|w| w.frame());
                log::debug!(
                    "macOS menu bar icon created (visible={}, win_frame={win_frame:?})",
                    status_item.isVisible()
                );
            }
            status_item.setMenu(Some(&menu));

            Ok(Tray {
                status_item,
                _menu: menu,
                _target: target,
                login,
            })
        }

        pub fn set_start_at_login(&self, on: bool) {
            self.login.setState(if on {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            NSStatusBar::systemStatusBar().removeStatusItem(&self.status_item);
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
            let data = crate::icon::app_icon_rgba(size)
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[3], p[0], p[1], p[2]])
                .collect();
            vec![ksni::Icon {
                width: size as i32,
                height: size as i32,
                data,
            }]
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
                    activate: Box::new(|t: &mut Sni| {
                        (t.on_command)(TrayCommand::ToggleStartAtLogin)
                    }),
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
            let handle = Sni {
                login: start_at_login,
                on_command,
            }
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
