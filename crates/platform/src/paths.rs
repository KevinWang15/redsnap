//! Where RedSnap keeps its files (PLAN.md §4.6). Resolved once at startup and
//! passed down; nothing else looks up directories.

use std::path::PathBuf;

use anyhow::{Context as _, Result};

#[derive(Clone, Debug)]
pub struct Paths {
    /// `config.json` (and its backup).
    pub config: PathBuf,
    /// `usage.db`, the clipboard store later, logs, the instance socket.
    pub data: PathBuf,
    /// Icon cache.
    pub cache: PathBuf,
}

impl Paths {
    /// The OS locations, or `<root>/{config,data,cache}` when `root` is given
    /// (tests and demo runs, so they never touch real settings).
    pub fn resolve(root: Option<PathBuf>) -> Result<Self> {
        let paths = match root {
            Some(r) => Paths {
                config: r.join("config"),
                data: r.join("data"),
                cache: r.join("cache"),
            },
            None => Paths {
                config: dirs::config_dir()
                    .context("no config directory")?
                    .join("redsnap"),
                data: dirs::data_local_dir()
                    .context("no data directory")?
                    .join("redsnap"),
                cache: dirs::cache_dir()
                    .context("no cache directory")?
                    .join("redsnap"),
            },
        };
        for dir in [&paths.config, &paths.data, &paths.cache, &paths.logs()] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        Ok(paths)
    }

    pub fn logs(&self) -> PathBuf {
        self.data.join("logs")
    }
}
