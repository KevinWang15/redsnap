//! Linux apps: XDG `.desktop` files (Desktop Entry Specification 1.5) from
//! `$XDG_DATA_HOME/applications` and every `$XDG_DATA_DIRS/*/applications`,
//! which includes Flatpak and Snap exports. The first file with a given
//! desktop id wins, so a user's own copy overrides the system one.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use redsnap_core::item::LaunchTarget;
use redsnap_core::providers::apps::AppEntry;

/// Files visited per root at most.
const MAX_FILES_PER_ROOT: usize = 20_000;

pub fn roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
    let dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or("/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(dirs.split(':').map(PathBuf::from))
        .map(|d| d.join("applications"))
        .chain(extra.iter().cloned())
        .collect()
}

pub fn scan(extra: &[PathBuf]) -> Result<Vec<AppEntry>> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktops: Vec<&str> = desktop.split(':').filter(|s| !s.is_empty()).collect();
    let locale = locale();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for root in roots(extra) {
        let mut files = Vec::new();
        walk(&root, &root, &mut files);
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if let Some(entry) = parse(&id, &path, &text, &locale, &desktops) {
                out.push(entry);
            }
        }
    }
    Ok(out)
}

/// Desktop ids: the path under the root with `/` → `-` (`kde4/foo.desktop` → `kde4-foo.desktop`).
fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if out.len() >= MAX_FILES_PER_ROOT {
            log::warn!("stopped reading {} at {MAX_FILES_PER_ROOT} files", root.display());
            return;
        }
        let path = e.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.extension().is_some_and(|x| x == "desktop")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push((rel.to_string_lossy().replace('/', "-"), path));
        }
    }
}

/// `zh_CN.UTF-8@x` → the lookup order `zh_CN`, `zh`.
fn locale() -> Vec<String> {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default();
    let base = raw.split(['.', '@']).next().unwrap_or("").to_string();
    let mut out = Vec::new();
    if !base.is_empty() && base != "C" && base != "POSIX" {
        if let Some((lang, _)) = base.split_once('_') {
            out.push(base.clone());
            out.push(lang.to_string());
        } else {
            out.push(base);
        }
    }
    out
}

/// Parses the `[Desktop Entry]` group. `None` for entries that shouldn't
/// appear: not an application, `NoDisplay`, `Hidden`, shown only in other
/// desktops, or a `TryExec` program that isn't installed.
pub fn parse(id: &str, path: &Path, text: &str, locale: &[String], desktops: &[&str]) -> Option<AppEntry> {
    let mut group = false;
    let mut keys: HashMap<String, String> = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            group = line == "[Desktop Entry]";
            continue;
        }
        if !group || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            keys.entry(k.trim().to_string()).or_insert_with(|| unescape(v.trim()));
        }
    }
    let get = |k: &str| keys.get(k).map(String::as_str);
    let localized = |k: &str| locale.iter().find_map(|l| get(&format!("{k}[{l}]"))).or(get(k));
    let truthy = |k: &str| get(k).is_some_and(|v| v == "true");
    if get("Type") != Some("Application") || truthy("NoDisplay") || truthy("Hidden") {
        return None;
    }
    let list = |k: &str| get(k).map(|v| v.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>());
    if let Some(only) = list("OnlyShowIn")
        && !only.iter().any(|d| desktops.contains(&d.as_str()))
    {
        return None;
    }
    if let Some(not) = list("NotShowIn")
        && not.iter().any(|d| desktops.contains(&d.as_str()))
    {
        return None;
    }
    if let Some(try_exec) = get("TryExec")
        && !on_path(try_exec)
    {
        return None;
    }
    let name = localized("Name")?.to_string();
    let mut aliases: Vec<String> = Vec::new();
    for extra in [get("Name"), localized("GenericName"), get("GenericName")].into_iter().flatten() {
        if extra != name && !aliases.iter().any(|a| a == extra) {
            aliases.push(extra.to_string());
        }
    }
    for kw in localized("Keywords").into_iter().chain(get("Keywords")).flat_map(|k| k.split(';')) {
        if !kw.is_empty() && !aliases.iter().any(|a| a == kw) {
            aliases.push(kw.to_string());
        }
    }
    // The program name (`code` for Visual Studio Code).
    if let Some(exec) = get("Exec")
        && let Some(program) = exec.split_whitespace().next()
        && let Some(stem) = Path::new(program.trim_matches('"')).file_name()
    {
        let stem = stem.to_string_lossy().to_string();
        // Launch wrappers say nothing about the app.
        const WRAPPERS: &[&str] = &["env", "sh", "bash", "flatpak", "snap", "sudo", "pkexec", "gtk-launch", "xdg-open"];
        if !aliases.contains(&stem) && stem != name && !WRAPPERS.contains(&stem.as_str()) {
            aliases.push(stem);
        }
    }
    let subtitle = localized("Comment").map(str::to_string).unwrap_or_else(|| path.display().to_string());
    Some(AppEntry {
        key: id.to_string(),
        name,
        aliases,
        subtitle,
        launch: LaunchTarget::Desktop { id: id.to_string(), file: path.to_path_buf() },
        location: Some(path.to_path_buf()),
    })
}

/// Desktop-entry string escapes: `\s \n \t \r \\`.
fn unescape(v: &str) -> String {
    let mut out = String::new();
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn on_path(program: &str) -> bool {
    let p = Path::new(program);
    if p.is_absolute() {
        return p.exists();
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(program).exists()))
}

/// The icon named by the desktop file's `Icon` key, from the icon theme (or an
/// absolute path), as straight BGRA scaled to `px`.
pub fn icon(target: &LaunchTarget, px: u32) -> Option<(usize, usize, Vec<u8>)> {
    let LaunchTarget::Desktop { file, .. } = target else { return None };
    let text = std::fs::read_to_string(file).ok()?;
    let name = text
        .lines()
        .skip_while(|l| l.trim() != "[Desktop Entry]")
        .find_map(|l| l.trim().strip_prefix("Icon="))?
        .trim()
        .to_string();
    let path = if Path::new(&name).is_absolute() { PathBuf::from(&name) } else { find_icon(&name, px)? };
    super::decode_image(&path, px)
}

/// Looks `name` up in the hicolor theme and pixmaps, preferring the size
/// nearest above `px`, then scalable SVGs.
fn find_icon(name: &str, px: u32) -> Option<PathBuf> {
    let bases: Vec<PathBuf> = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .into_iter()
        .chain(std::env::var("XDG_DATA_DIRS").unwrap_or("/usr/local/share:/usr/share".into()).split(':').map(PathBuf::from))
        .collect();
    let mut sizes: Vec<u32> = vec![16, 22, 24, 32, 48, 64, 96, 128, 256, 512];
    sizes.sort_by_key(|s| if *s >= px { s - px } else { 10_000 + px - s });
    for base in &bases {
        for theme in ["hicolor"] {
            for s in &sizes {
                let p = base.join(format!("icons/{theme}/{s}x{s}/apps/{name}.png"));
                if p.exists() {
                    return Some(p);
                }
            }
            let svg = base.join(format!("icons/{theme}/scalable/apps/{name}.svg"));
            if svg.exists() {
                return Some(svg);
            }
        }
        for ext in ["png", "svg", "xpm"] {
            let p = base.join(format!("pixmaps/{name}.{ext}"));
            if p.exists() && ext != "xpm" {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str, desktops: &[&str]) -> Option<AppEntry> {
        parse("org.example.App.desktop", Path::new("/usr/share/applications/org.example.App.desktop"), text, &["zh_CN".into(), "zh".into()], desktops)
    }

    #[test]
    fn localized_names_keywords_and_program() {
        let e = p(
            "[Desktop Entry]\nType=Application\nName=Files\nName[zh_CN]=文件\nGenericName=File Manager\nKeywords=folder;manager;\nExec=nautilus --new-window %U\nComment=Access files\n[Desktop Action new]\nName=New Window\n",
            &[],
        )
        .unwrap();
        assert_eq!(e.name, "文件");
        assert_eq!(e.aliases, ["Files", "File Manager", "folder", "manager", "nautilus"]);
        assert_eq!(e.subtitle, "Access files");
        assert!(matches!(e.launch, LaunchTarget::Desktop { ref id, .. } if id == "org.example.App.desktop"));
    }

    #[test]
    fn hidden_and_other_desktop_entries_are_skipped() {
        assert!(p("[Desktop Entry]\nType=Application\nName=X\nNoDisplay=true\n", &[]).is_none());
        assert!(p("[Desktop Entry]\nType=Link\nName=X\nURL=https://x\n", &[]).is_none());
        assert!(p("[Desktop Entry]\nType=Application\nName=X\nOnlyShowIn=KDE;\n", &["GNOME"]).is_none());
        assert!(p("[Desktop Entry]\nType=Application\nName=X\nOnlyShowIn=KDE;\n", &["KDE"]).is_some());
        assert!(p("[Desktop Entry]\nType=Application\nName=X\nNotShowIn=GNOME;\n", &["GNOME"]).is_none());
        assert!(p("[Desktop Entry]\nType=Application\nName=X\nTryExec=/nonexistent/redsnap-test\n", &[]).is_none());
    }

    #[test]
    fn escapes() {
        assert_eq!(unescape(r"a\sb\\c\nd"), "a b\\c\nd");
    }
}
