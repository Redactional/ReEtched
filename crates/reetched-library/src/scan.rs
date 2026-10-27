//! Folder walking plus the scanner thread that turns file-system notifications into
//! debounced, incremental listings for `Library::poll` to apply.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Quiet time after the last notification before scanning (copies fire many events).
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Scan anyway after this long, so a folder that never goes quiet still updates.
const MAX_DELAY: Duration = Duration::from_secs(2);
/// Re-walk interval for roots the OS watcher couldn't take.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

pub(crate) struct FileInfo {
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

/// Everything at or under `prefix` ("" = whole root) as it is on disk now.
pub(crate) struct ScanResult {
    pub root: String,
    pub prefix: String,
    pub files: Vec<FileInfo>,
}

enum Cmd {
    Changed(PathBuf),
    RescanAll,
    AddRoot {
        name: String,
        path: PathBuf,
        poll: bool,
    },
    RemoveRoot(String),
}

/// `start` relative to `root` as a `/`-separated string; `None` outside the root, for
/// the root itself, or for anything hidden (dot files such as the metadata file).
pub(crate) fn rel_string(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rel
        .components()
        .map(|c| c.as_os_str().to_str())
        .collect::<Option<_>>()?;
    if parts.is_empty() || parts.iter().any(|p| p.starts_with('.')) {
        return None;
    }
    Some(parts.join("/"))
}

/// Files at or under `start` (a file or folder inside `root`). Missing paths yield nothing.
pub(crate) fn walk(root: &Path, start: &Path) -> Vec<FileInfo> {
    walkdir::WalkDir::new(start)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            Some(FileInfo {
                rel: rel_string(root, e.path())?,
                path: e.path().to_path_buf(),
                size: md.len(),
                modified: md.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            })
        })
        .collect()
}

pub(crate) struct Scanner {
    cmds: Sender<Cmd>,
    pub results: Receiver<ScanResult>,
    watcher: Option<RecommendedWatcher>,
}

impl Scanner {
    pub fn new() -> Self {
        let (cmds, cmd_rx) = mpsc::channel();
        let (result_tx, results) = mpsc::channel();
        thread::Builder::new()
            .name("library-scanner".into())
            .spawn(move || run(cmd_rx, result_tx))
            .expect("spawning library scanner thread");

        let tx = cmds.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let _ = match res {
                Ok(ev) if ev.need_rescan() => tx.send(Cmd::RescanAll),
                Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => Ok(()),
                Ok(ev) => ev
                    .paths
                    .into_iter()
                    .try_for_each(|p| tx.send(Cmd::Changed(p))),
                // Lost events: the only safe answer is a full rescan.
                Err(_) => tx.send(Cmd::RescanAll),
            };
        });
        let watcher = watcher
            .inspect_err(|e| log::warn!("file watcher unavailable: {e}"))
            .ok();
        Self {
            cmds,
            results,
            watcher,
        }
    }

    /// Starts watching `path`; on failure the root is polled instead and the error returned.
    pub fn add_root(&mut self, name: &str, path: &Path) -> Result<(), String> {
        let watched = match &mut self.watcher {
            Some(w) => w
                .watch(path, RecursiveMode::Recursive)
                .map_err(|e| e.to_string()),
            None => Err("no file watcher on this platform".into()),
        };
        let _ = self.cmds.send(Cmd::AddRoot {
            name: name.into(),
            path: path.into(),
            poll: watched.is_err(),
        });
        watched
    }

    pub fn remove_root(&mut self, name: &str, path: &Path) {
        if let Some(w) = &mut self.watcher {
            let _ = w.unwatch(path);
        }
        let _ = self.cmds.send(Cmd::RemoveRoot(name.into()));
    }
}

struct RootEntry {
    name: String,
    path: PathBuf,
    /// `path` with symlinks resolved: watchers may report that form instead
    /// (macOS FSEvents gives /private/var/... for /var/...).
    real: PathBuf,
    poll: bool,
}

impl RootEntry {
    /// `p` (from an event, in either form) as a path under `self.path`, if it is inside.
    fn local(&self, p: &Path) -> Option<PathBuf> {
        if p.starts_with(&self.path) {
            return Some(p.to_owned());
        }
        p.strip_prefix(&self.real)
            .ok()
            .map(|rel| self.path.join(rel))
    }
}

fn run(cmds: Receiver<Cmd>, out: Sender<ScanResult>) {
    let mut roots: Vec<RootEntry> = Vec::new();
    let mut pending: Vec<PathBuf> = Vec::new();
    let mut first_event: Option<Instant> = None;
    let mut last_event = Instant::now();
    let mut next_poll = Instant::now() + POLL_INTERVAL;
    loop {
        let deadline = match first_event {
            Some(first) => (last_event + DEBOUNCE)
                .min(first + MAX_DELAY)
                .min(next_poll),
            None => next_poll,
        };
        let mut touched = |pending: &mut Vec<PathBuf>, paths: &mut dyn Iterator<Item = PathBuf>| {
            pending.extend(paths);
            last_event = Instant::now();
            first_event.get_or_insert(last_event);
        };
        match cmds.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Cmd::Changed(p)) => touched(&mut pending, &mut std::iter::once(p)),
            Ok(Cmd::RescanAll) => touched(&mut pending, &mut roots.iter().map(|r| r.path.clone())),
            Ok(Cmd::AddRoot { name, path, poll }) => {
                let real = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
                roots.push(RootEntry {
                    name,
                    path,
                    real,
                    poll,
                })
            }
            Ok(Cmd::RemoveRoot(name)) => roots.retain(|r| r.name != name),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let now = Instant::now();
        if let Some(first) = first_event
            && (now >= last_event + DEBOUNCE || now >= first + MAX_DELAY)
        {
            first_event = None;
            if !flush(&roots, std::mem::take(&mut pending), &out) {
                return;
            }
        }
        if now >= next_poll {
            next_poll = now + POLL_INTERVAL;
            // ponytail: polled roots are fully re-walked (stat only, no reads); fine for
            // thousands of files, add per-folder mtime skipping for huge network shares.
            let polled: Vec<PathBuf> = roots
                .iter()
                .filter(|r| r.poll)
                .map(|r| r.path.clone())
                .collect();
            if !polled.is_empty() && !flush(&roots, polled, &out) {
                return;
            }
        }
    }
}

/// Walks each changed path (skipping those under another changed path). False once the
/// library is gone.
fn flush(roots: &[RootEntry], mut paths: Vec<PathBuf>, out: &Sender<ScanResult>) -> bool {
    paths.sort();
    paths.dedup();
    let mut done: Vec<PathBuf> = Vec::new();
    for path in paths {
        // Innermost root wins when roots nest.
        let Some((root, path)) = roots
            .iter()
            .filter_map(|r| Some((r, r.local(&path)?)))
            .max_by_key(|(r, _)| r.path.components().count())
        else {
            continue;
        };
        if done.iter().any(|d| path.starts_with(d)) {
            continue;
        }
        let prefix = if path == root.path {
            String::new()
        } else {
            match rel_string(&root.path, &path) {
                Some(rel) => rel,
                None => continue, // hidden file, e.g. our own metadata file being saved
            }
        };
        let files = walk(&root.path, &path);
        if out
            .send(ScanResult {
                root: root.name.clone(),
                prefix,
                files,
            })
            .is_err()
        {
            return false;
        }
        done.push(path);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_under_the_resolved_root_map_back() {
        let root = RootEntry {
            name: "W".into(),
            path: "/var/lib/w".into(),
            real: "/private/var/lib/w".into(),
            poll: false,
        };
        let want = Some(PathBuf::from("/var/lib/w/textures/new.png"));
        assert_eq!(root.local(Path::new("/var/lib/w/textures/new.png")), want);
        assert_eq!(
            root.local(Path::new("/private/var/lib/w/textures/new.png")),
            want
        );
        assert_eq!(root.local(Path::new("/elsewhere/new.png")), None);
    }
}
