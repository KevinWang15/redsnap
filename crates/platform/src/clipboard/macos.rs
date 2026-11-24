//! macOS clipboard capture, write-back, and pasting into the previous app
//! (docs/clipboard.md).
//!
//! macOS has no pasteboard change notification, so a background thread polls
//! `NSPasteboard.generalPasteboard.changeCount` every 150 ms. If the count
//! jumped by more than one between polls, a `Skip::Replaced` entry records the
//! gap so nothing disappears silently.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, msg_send};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPasteboard,
    NSPasteboardType, NSPasteboardWriting, NSRunningApplication, NSWorkspace,
};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSString, NSURL};
use redsnap_core::clipboard::{Capture, Content, Skip};

use super::convert::{png_size, strip_html};
use super::{ClipEvent, ClipEventKind, OnClip, ReadLimits, Target, now_ms};

const POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Marker attached to RedSnap's own pasteboard writes so the watcher reports
/// them as `ClipEventKind::OwnWrite` instead of adding a duplicate entry.
const OWN_TYPE: &str = "com.kevinwang15.redsnap.write";
const TYPE_FILE_URL: &str = "public.file-url";
const TYPE_TEXT: &str = "public.utf8-plain-text";
const TYPE_HTML: &str = "public.html";
const TYPE_PNG: &str = "public.png";
const TYPE_TIFF: &str = "public.tiff";

/// Standard macOS privacy markers honored by password managers and snippet tools.
const PRIVATE_TYPES: &[&str] = &[
    "org.nspasteboard.ConcealedType",
    "org.nspasteboard.TransientType",
    "com.agilebits.onepassword",
];

/// Tag placed in `kCGEventSourceUserData` on synthetic keystrokes ("RSNP") so
/// RedSnap's snippet event tap ignores its own events.
pub(crate) const EVENT_MARKER: i64 = 0x5253_4E50;

/// Virtual keycode for 'V' on macOS (`kVK_ANSI_V`).
const VK_V: u16 = 9;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
}

/// Whether macOS Accessibility permission has been granted to this process
/// (required to post synthetic `Cmd+V` and snippet keystrokes).
pub fn accessibility_trusted() -> bool {
    unsafe { AXIsProcessTrusted() }
}

pub fn can_paste() -> bool {
    accessibility_trusted()
}

pub struct Watcher {
    stop: Arc<AtomicBool>,
    write_lock: Mutex<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    pub fn start(limits: ReadLimits, on_clip: OnClip) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = stop.clone();
        let initial_count = autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount());
        let handle = thread::Builder::new()
            .name("clipboard-watcher".into())
            .spawn(move || {
                let mut last_count = initial_count;
                while !stop_flag.load(Ordering::Relaxed) {
                    thread::sleep(POLL_INTERVAL);
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    autoreleasepool(|_| {
                        let pb = NSPasteboard::generalPasteboard();
                        let count = pb.changeCount();
                        if count == last_count {
                            return;
                        }
                        let diff = count.saturating_sub(last_count);
                        last_count = count;
                        let at = now_ms();
                        let source_app = frontmost_app_name();
                        if diff > 1 {
                            on_clip(ClipEvent {
                                at,
                                source_app: source_app.clone(),
                                kind: ClipEventKind::Captured(Capture::Skipped(Skip::Replaced {
                                    count: (diff - 1) as u64,
                                })),
                            });
                        }
                        let kind = read_pasteboard(&pb, &limits);
                        on_clip(ClipEvent {
                            at,
                            source_app,
                            kind,
                        });
                    });
                }
            })?;
        Ok(Watcher {
            stop,
            write_lock: Mutex::new(()),
            thread: Some(handle),
        })
    }

    pub fn write(&self, content: &Content, id: &str) -> Result<()> {
        let _guard = self.write_lock.lock().unwrap();
        autoreleasepool(|_| {
            let pb = NSPasteboard::generalPasteboard();
            write_pasteboard(&pb, content, id)
        })
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub(crate) fn frontmost_app_name() -> Option<String> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        app.localizedName().map(|s| s.to_string())
    })
}

/// The clipboard's current plain text, for the `{{clipboard}}` snippet placeholder.
pub(crate) fn current_text() -> Option<String> {
    autoreleasepool(|_| {
        let pb = NSPasteboard::generalPasteboard();
        let ty = NSString::from_str(TYPE_TEXT);
        let data = pb.dataForType(&ty)?;
        if data.len() > (4 << 20) {
            return None;
        }
        String::from_utf8(data.to_vec()).ok()
    })
}

fn has_type(types: &[String], want: &str) -> bool {
    types.iter().any(|t| t == want)
}

fn pasteboard_types(pb: &NSPasteboard) -> Vec<String> {
    pb.types()
        .map(|arr| arr.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default()
}

fn read_file_urls(pb: &NSPasteboard) -> Vec<PathBuf> {
    let Some(items) = pb.pasteboardItems() else {
        return Vec::new();
    };
    let ty = NSString::from_str(TYPE_FILE_URL);
    let mut files = Vec::new();
    for item in items.iter() {
        let Some(raw) = item.stringForType(&ty) else {
            continue;
        };
        let Some(url) = NSURL::URLWithString(&raw) else {
            continue;
        };
        if !url.isFileURL() {
            continue;
        }
        // Finder writes file reference URLs (`file:///.file/id=...`); resolve
        // them to standard POSIX file paths.
        let resolved = url.filePathURL().unwrap_or(url);
        if let Some(path) = resolved.to_file_path() {
            files.push(path);
        }
        if files.len() >= 10_000 {
            break;
        }
    }
    files
}

fn tiff_to_png(tiff: &[u8]) -> Option<Vec<u8>> {
    let data = NSData::from_vec(tiff.to_vec());
    let rep = NSBitmapImageRep::imageRepWithData(&data)?;
    let empty: Retained<
        NSDictionary<objc2_app_kit::NSBitmapImageRepPropertyKey, objc2::runtime::AnyObject>,
    > = NSDictionary::new();
    let png =
        unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &empty) }?;
    Some(png.to_vec())
}

fn png_to_tiff(png: &[u8]) -> Option<Vec<u8>> {
    let data = NSData::from_vec(png.to_vec());
    let img = NSImage::initWithData(NSImage::alloc(), &data)?;
    let tiff = img.TIFFRepresentation()?;
    Some(tiff.to_vec())
}

fn read_pasteboard(pb: &NSPasteboard, limits: &ReadLimits) -> ClipEventKind {
    let skip = |s| ClipEventKind::Captured(Capture::Skipped(s));
    let types = pasteboard_types(pb);

    let own_ty = NSString::from_str(OWN_TYPE);
    if let Some(id) = pb.stringForType(&own_ty) {
        return ClipEventKind::OwnWrite(id.to_string());
    }

    for marker in PRIVATE_TYPES {
        if has_type(&types, marker) {
            return skip(Skip::Private((*marker).to_string()));
        }
    }

    // Files first: Finder also puts file names as plain text.
    if has_type(&types, TYPE_FILE_URL) {
        let files = read_file_urls(pb);
        if !files.is_empty() {
            return ClipEventKind::Captured(Capture::Content(Content::Files(files)));
        }
    }

    let html_ty = NSString::from_str(TYPE_HTML);
    let html = pb.dataForType(&html_ty).and_then(|d| {
        (d.len() <= limits.max_text_bytes)
            .then(|| String::from_utf8_lossy(&d.to_vec()).into_owned())
    });

    // Text before images: spreadsheets and browsers often attach an image preview too.
    let text_ty = NSString::from_str(TYPE_TEXT);
    if let Some(data) = pb.dataForType(&text_ty) {
        if data.len() > limits.max_text_bytes {
            return skip(Skip::TooLarge {
                bytes: data.len() as u64,
            });
        }
        let text = String::from_utf8_lossy(&data.to_vec()).into_owned();
        if !text.is_empty() {
            return ClipEventKind::Captured(Capture::Content(Content::Text { text, html }));
        }
    }

    let png_ty = NSString::from_str(TYPE_PNG);
    if let Some(data) = pb.dataForType(&png_ty) {
        if data.len() > limits.max_image_bytes {
            return skip(Skip::TooLarge {
                bytes: data.len() as u64,
            });
        }
        let png = data.to_vec();
        if let Some((width, height)) = png_size(&png) {
            return ClipEventKind::Captured(Capture::Content(Content::Image {
                png,
                width,
                height,
            }));
        }
    }

    let tiff_ty = NSString::from_str(TYPE_TIFF);
    if let Some(data) = pb.dataForType(&tiff_ty) {
        if data.len() > limits.max_image_bytes {
            return skip(Skip::TooLarge {
                bytes: data.len() as u64,
            });
        }
        let tiff = data.to_vec();
        if let Some(png) = tiff_to_png(&tiff) {
            if png.len() > limits.max_image_bytes {
                return skip(Skip::TooLarge {
                    bytes: png.len() as u64,
                });
            }
            if let Some((width, height)) = png_size(&png) {
                return ClipEventKind::Captured(Capture::Content(Content::Image {
                    png,
                    width,
                    height,
                }));
            }
        }
    }

    if let Some(h) = html {
        let text = strip_html(&h);
        if !text.trim().is_empty() {
            return ClipEventKind::Captured(Capture::Content(Content::Text {
                text,
                html: Some(h),
            }));
        }
    }

    let names = if types.is_empty() {
        "empty".to_string()
    } else {
        types.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
    };
    skip(Skip::Unsupported(names))
}

fn write_pasteboard(pb: &NSPasteboard, content: &Content, id: &str) -> Result<()> {
    pb.clearContents();
    match content {
        Content::Text { text, html } => {
            let text_ty = NSString::from_str(TYPE_TEXT);
            let ok = pb.setString_forType(&NSString::from_str(text), &text_ty);
            anyhow::ensure!(ok, "NSPasteboard refused plain text");
            if let Some(h) = html {
                let html_ty = NSString::from_str(TYPE_HTML);
                let data = NSData::from_vec(h.as_bytes().to_vec());
                let _ = pb.setData_forType(Some(&data), &html_ty);
            }
        }
        Content::Image { png, .. } => {
            let png_ty = NSString::from_str(TYPE_PNG);
            let data = NSData::from_vec(png.clone());
            let ok = pb.setData_forType(Some(&data), &png_ty);
            anyhow::ensure!(ok, "NSPasteboard refused PNG image");
            if let Some(tiff) = png_to_tiff(png) {
                let tiff_ty = NSString::from_str(TYPE_TIFF);
                let tdata = NSData::from_vec(tiff);
                let _ = pb.setData_forType(Some(&tdata), &tiff_ty);
            }
        }
        Content::Files(paths) => {
            let urls: Vec<Retained<NSURL>> = paths
                .iter()
                .map(|p| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy())))
                .collect();
            let writers: Vec<&ProtocolObject<dyn NSPasteboardWriting>> = urls
                .iter()
                .map(|u| ProtocolObject::from_ref(&**u))
                .collect();
            let arr = NSArray::from_slice(&writers);
            let ok = pb.writeObjects(&arr);
            anyhow::ensure!(ok, "NSPasteboard refused file URLs");
        }
    }
    let own_ty: Retained<NSPasteboardType> = NSString::from_str(OWN_TYPE);
    let own_types = NSArray::from_slice(&[&*own_ty]);
    let _ = unsafe { pb.addTypes_owner(&own_types, None) };
    let _ = pb.setString_forType(&NSString::from_str(id), &own_ty);
    Ok(())
}

/// The frontmost application's PID before RedSnap took focus (ignoring RedSnap itself).
pub fn foreground() -> Option<Target> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        if pid <= 0 || pid as u32 == std::process::id() {
            return None;
        }
        Some(Target(pid as usize))
    })
}

/// Reactivates `target` (by PID) and waits up to 500 ms for it to become the
/// frontmost application.
pub(crate) fn activate(target: Target) -> Result<()> {
    let pid = target.0 as i32;
    let activated = autoreleasepool(|_| -> Result<()> {
        let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) else {
            bail!("the app has closed");
        };
        if app.isTerminated() {
            bail!("the app has closed");
        }
        #[allow(deprecated)]
        let ok = app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        if !ok {
            // Fallback for macOS 14+ where `activate` may succeed even when the
            // deprecated option returns false under cooperative activation.
            let _: bool = unsafe { msg_send![&*app, activate] };
        }
        Ok(())
    });
    activated?;

    for _ in 0..50 {
        let front = autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .map(|a| a.processIdentifier())
        });
        if front == Some(pid) {
            thread::sleep(Duration::from_millis(30));
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
    bail!("the app didn't come back to the front")
}

pub fn paste_into(target: Target) -> Result<()> {
    if !accessibility_trusted() {
        bail!(
            "Accessibility permission is needed to paste automatically; the entry is on the clipboard"
        );
    }
    activate(target).map_err(|e| anyhow::anyhow!("{e}; the entry is on the clipboard"))?;

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState);
    let Some(down) = CGEvent::new_keyboard_event(source.as_deref(), VK_V, true) else {
        bail!("couldn't create Cmd+V key-down event; the entry is on the clipboard");
    };
    let Some(up) = CGEvent::new_keyboard_event(source.as_deref(), VK_V, false) else {
        bail!("couldn't create Cmd+V key-up event; the entry is on the clipboard");
    };
    CGEvent::set_flags(Some(&down), CGEventFlags::MaskCommand);
    CGEvent::set_flags(Some(&up), CGEventFlags::MaskCommand);
    CGEvent::set_integer_value_field(Some(&down), CGEventField::EventSourceUserData, EVENT_MARKER);
    CGEvent::set_integer_value_field(Some(&up), CGEventField::EventSourceUserData, EVENT_MARKER);

    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&down));
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&up));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ReadLimits {
        ReadLimits {
            max_text_bytes: 4 << 20,
            max_image_bytes: 64 << 20,
        }
    }

    fn tiny_png() -> Vec<u8> {
        let img = image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn unique_pasteboard_captures_text_html_images_files_and_skips() {
        autoreleasepool(|_| {
            let pb = NSPasteboard::pasteboardWithUniqueName();

            // 1. Plain text + HTML
            pb.clearContents();
            pb.setString_forType(
                &NSString::from_str("hello mac"),
                &NSString::from_str(TYPE_TEXT),
            );
            pb.setData_forType(
                Some(&NSData::from_vec(b"<b>hello mac</b>".to_vec())),
                &NSString::from_str(TYPE_HTML),
            );
            assert!(matches!(
                read_pasteboard(&pb, &limits()),
                ClipEventKind::Captured(Capture::Content(Content::Text { ref text, html: Some(ref h) }))
                if text == "hello mac" && h == "<b>hello mac</b>"
            ));

            // 2. Private marker is skipped with marker name
            pb.clearContents();
            pb.setString_forType(
                &NSString::from_str("secret"),
                &NSString::from_str(TYPE_TEXT),
            );
            pb.setData_forType(
                Some(&NSData::new()),
                &NSString::from_str("org.nspasteboard.ConcealedType"),
            );
            assert!(matches!(
                read_pasteboard(&pb, &limits()),
                ClipEventKind::Captured(Capture::Skipped(Skip::Private(ref m)))
                if m == "org.nspasteboard.ConcealedType"
            ));

            // 3. Size limit enforced before storing
            pb.clearContents();
            pb.setString_forType(
                &NSString::from_str("0123456789"),
                &NSString::from_str(TYPE_TEXT),
            );
            let small = ReadLimits {
                max_text_bytes: 4,
                max_image_bytes: 4,
            };
            assert!(matches!(
                read_pasteboard(&pb, &small),
                ClipEventKind::Captured(Capture::Skipped(Skip::TooLarge { bytes: 10 }))
            ));

            // 4. TIFF image is converted to PNG
            let png = tiny_png();
            let tiff = png_to_tiff(&png).expect("png -> tiff");
            pb.clearContents();
            pb.setData_forType(
                Some(&NSData::from_vec(tiff)),
                &NSString::from_str(TYPE_TIFF),
            );
            assert!(matches!(
                read_pasteboard(&pb, &limits()),
                ClipEventKind::Captured(Capture::Content(Content::Image {
                    width: 2,
                    height: 1,
                    ..
                }))
            ));

            // 5. OwnWrite round-trip for Text, Image, and Files
            for (id, content) in [
                (
                    "entry-text",
                    Content::Text {
                        text: "written".into(),
                        html: Some("<i>written</i>".into()),
                    },
                ),
                (
                    "entry-img",
                    Content::Image {
                        png: tiny_png(),
                        width: 2,
                        height: 1,
                    },
                ),
                ("entry-files", Content::Files(vec![PathBuf::from("/tmp")])),
            ] {
                write_pasteboard(&pb, &content, id).unwrap();
                assert!(
                    matches!(read_pasteboard(&pb, &limits()), ClipEventKind::OwnWrite(ref got) if got == id),
                    "expected OwnWrite({id})"
                );
            }

            let _: () = unsafe { msg_send![&*pb, releaseGlobally] };
        });
    }
}
