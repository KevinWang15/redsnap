//! Linux: the `plocate` index (or the older `locate`), which the system's
//! `updatedb` keeps current. Names only (`-b`), any case, every word.

use std::path::{Path, PathBuf};
use std::process::Command;

use redsnap_core::files::Outcome;

pub const BACKEND: &str = "plocate";
pub const SYNTAX: &str = "Type part of a name; every word must match. Wildcards work: *.pdf";

/// The first of `names` on `PATH`.
fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    names.iter().find_map(|n| {
        std::env::split_paths(&path)
            .map(|d| d.join(n))
            .find(|p| p.is_file())
    })
}

fn not_installed() -> Outcome {
    Outcome::Unavailable {
        title: "File search needs plocate".into(),
        subtitle: "Install plocate with your package manager; it indexes files daily".into(),
        url: Some("https://plocate.sesse.net/".into()),
    }
}

/// The arguments for `words`: `plocate` matches every word already; the
/// older `locate` needs `-A` for that.
fn args(program: &Path, words: &str, limit: usize) -> Vec<String> {
    let mut args = vec![
        "-i".to_string(),
        "-b".into(),
        "-0".into(),
        "-l".into(),
        limit.to_string(),
    ];
    if program.file_name().is_some_and(|n| n == "locate") {
        args.push("-A".into());
    }
    args.push("--".into());
    args.extend(words.split_whitespace().map(str::to_string));
    args
}

pub fn search(words: &str, limit: usize) -> Outcome {
    let Some(program) = on_path(&["plocate", "locate"]) else {
        return not_installed();
    };
    if words.trim().is_empty() {
        return Outcome::Hits(Vec::new());
    }
    let out = match Command::new(&program)
        .args(args(&program, words, limit))
        .output()
    {
        Ok(o) => o,
        Err(e) => return Outcome::Failed(format!("{}: {e}", program.display())),
    };
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Exit status 1 with nothing on stderr: no matches.
    if !out.status.success() && !stderr.trim().is_empty() {
        let line = stderr.lines().next().unwrap_or_default().to_string();
        if line.contains("No such file") || line.contains(".db") {
            return Outcome::Unavailable {
                title: "The file index isn't built yet".into(),
                subtitle: "Run sudo updatedb once; it's kept current after that".into(),
                url: None,
            };
        }
        return Outcome::Failed(line);
    }
    let paths = out
        .stdout
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            use std::os::unix::ffi::OsStrExt;
            PathBuf::from(std::ffi::OsStr::from_bytes(p))
        });
    Outcome::Hits(super::hits_for(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments() {
        assert_eq!(
            args(Path::new("/usr/bin/plocate"), " q3  report ", 50),
            ["-i", "-b", "-0", "-l", "50", "--", "q3", "report"]
        );
        assert_eq!(
            args(Path::new("/usr/bin/locate"), "-x", 5),
            ["-i", "-b", "-0", "-l", "5", "-A", "--", "-x"]
        );
    }
}
