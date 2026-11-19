//! macOS: Spotlight, through `mdfind`. Matches names (any case, any
//! accents) containing every word.

use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use redsnap_core::files::Outcome;

pub const BACKEND: &str = "Spotlight";
pub const SYNTAX: &str = "Type part of a name; every word must match";

/// A Spotlight query for names containing every word. Quotes, backslashes
/// and `*` in a word are matched as typed.
fn query(words: &str) -> String {
    words
        .split_whitespace()
        .map(|w| {
            let mut escaped = String::new();
            for c in w.chars() {
                if matches!(c, '"' | '\\' | '*') {
                    escaped.push('\\');
                }
                escaped.push(c);
            }
            format!("kMDItemFSName == \"*{escaped}*\"cd")
        })
        .collect::<Vec<_>>()
        .join(" && ")
}

pub fn search(words: &str, limit: usize) -> Outcome {
    if words.trim().is_empty() {
        return Outcome::Hits(Vec::new());
    }
    let mut child = match Command::new("/usr/bin/mdfind")
        .arg("-0")
        .arg(query(words))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(format!("mdfind: {e}")),
    };
    // Read until `limit` paths, then stop it: a short word matches a lot.
    let mut stdout = child.stdout.take().expect("piped");
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 16 * 1024]);
    loop {
        match stdout.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if buf.iter().filter(|&&b| b == 0).count() >= limit {
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let paths = buf.split(|&b| b == 0).filter(|p| !p.is_empty()).take(limit).map(|p| {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(p))
    });
    Outcome::Hits(super::hits_for(paths))
}

#[cfg(test)]
mod tests {
    #[test]
    fn query() {
        assert_eq!(super::query("q3 report"), r#"kMDItemFSName == "*q3*"cd && kMDItemFSName == "*report*"cd"#);
        assert_eq!(super::query(r#"a"b*\"#), r#"kMDItemFSName == "*a\"b\*\\*"cd"#);
    }
}
