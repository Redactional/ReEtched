//! Autosave and crash recovery.
//!
//! Each running ReEtched holds an exclusive lock on its own file in
//! `sessions/`, naming its autosave. A lock file nobody holds belongs to a
//! session that ended without cleaning up, i.e. crashed: its autosave is offered
//! back on the next start.

use crate::settings::config_dir;
use reetched_doc::{Document, Resources, project};
use reetched_io::Mesh;
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Seek, Write},
    path::{Path, PathBuf},
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime},
};

#[derive(Serialize, Deserialize)]
struct LockInfo {
    pid: u32,
    autosave: PathBuf,
    /// The project the autosave belongs to, if it has been saved somewhere.
    project: Option<PathBuf>,
}

/// An autosave left by a session that crashed.
pub struct Recovery {
    pub autosave: PathBuf,
    pub project: Option<PathBuf>,
    pub saved_at: SystemTime,
    lock: PathBuf,
}

impl Recovery {
    /// Deletes the crashed session's files (after recovering or discarding).
    pub fn forget(&self) {
        let _ = std::fs::remove_file(&self.autosave);
        let _ = std::fs::remove_file(&self.lock);
    }
}

pub struct Session {
    lock_path: PathBuf,
    lock: Option<File>,
    autosave: PathBuf,
    last: Instant,
    /// The document as last autosaved, to skip saving when nothing changed.
    saved: Option<Document>,
    job: Option<JoinHandle<anyhow::Result<()>>>,
    pub recovery: Option<Recovery>,
}

fn sessions_dir() -> PathBuf {
    config_dir().join("sessions")
}

impl Session {
    /// Claims a lock for this session and looks for crashed ones.
    pub fn start() -> Self {
        let dir = sessions_dir();
        let _ = std::fs::create_dir_all(&dir);
        let recovery = find_crashed(&dir);
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let id = format!("{}-{stamp}", std::process::id());
        let lock_path = dir.join(format!("{id}.lock"));
        let autosave = dir.join(format!("{id}.retx"));
        let mut session = Session {
            lock_path,
            lock: None,
            autosave,
            last: Instant::now(),
            saved: None,
            job: None,
            recovery,
        };
        session.lock = File::create(&session.lock_path).ok().filter(|f| {
            // Held until this process exits; the OS releases it even on a crash.
            f.try_lock().is_ok()
        });
        session.write_lock(None);
        session
    }

    fn write_lock(&mut self, project: Option<&Path>) {
        let info = LockInfo {
            pid: std::process::id(),
            autosave: self.autosave.clone(),
            project: project.map(Path::to_owned),
        };
        if let (Some(f), Ok(json)) = (&mut self.lock, serde_json::to_vec(&info)) {
            let _ = f.set_len(0);
            let _ = f.rewind();
            let _ = f.write_all(&json);
            let _ = f.flush();
        }
    }

    /// Autosaves in the background when `minutes` have passed and the document
    /// changed. Returns an error from the previous autosave, if it failed.
    pub fn tick(
        &mut self,
        minutes: f32,
        doc: &Document,
        mesh: &Mesh,
        resources: &Resources,
        project: Option<&Path>,
    ) -> Option<String> {
        let mut error = None;
        if self.job.as_ref().is_some_and(|j| j.is_finished()) {
            match self.job.take().map(|j| j.join()) {
                Some(Ok(Err(e))) => error = Some(format!("Autosave failed: {e:#}")),
                Some(Err(_)) => error = Some("Autosave failed".into()),
                _ => {}
            }
        }
        if minutes <= 0.0
            || self.job.is_some()
            || self.last.elapsed() < Duration::from_secs_f32(minutes * 60.0)
        {
            return error;
        }
        self.last = Instant::now();
        if self.saved.as_ref() == Some(doc) {
            return error;
        }
        self.write_lock(project);
        self.saved = Some(doc.clone());
        let (doc, mesh, resources) = (doc.clone(), mesh.clone(), resources.clone());
        let path = self.autosave.clone();
        self.job = Some(std::thread::spawn(move || {
            project::save(&path, &doc, &mesh, &resources)
        }));
        error
    }

    /// The document was saved (or a new one loaded): it's the baseline again.
    pub fn mark_saved(&mut self, doc: &Document, project: Option<&Path>) {
        self.saved = Some(doc.clone());
        self.last = Instant::now();
        self.write_lock(project);
    }

    /// A new document that was never saved: dirty once edited.
    pub fn mark_new(&mut self) {
        self.saved = None;
    }

    /// Clean exit: nothing to recover.
    pub fn end(&mut self) {
        if let Some(job) = self.job.take() {
            let _ = job.join();
        }
        if let Some(f) = self.lock.take() {
            let _ = f.unlock();
        }
        let _ = std::fs::remove_file(&self.autosave);
        let _ = std::fs::remove_file(&self.lock_path);
    }

    /// Changed since the last save (or, never saved, since it was created).
    pub fn is_dirty(&self, doc: &Document, edited: bool) -> bool {
        self.saved.as_ref().map_or(edited, |s| s != doc)
    }

    /// Where this session autosaves (for tests).
    #[cfg(test)]
    pub fn autosave_path(&self) -> &Path {
        &self.autosave
    }
}

/// The newest autosave of a session whose lock nobody holds. Stale locks
/// without an autosave are removed.
fn find_crashed(dir: &Path) -> Option<Recovery> {
    let mut found: Option<Recovery> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(mut f) = File::options().read(true).write(true).open(&path) else {
            continue;
        };
        if f.try_lock().is_err() {
            continue; // another ReEtched is running with it
        }
        // Through the locked handle: Windows refuses reads of a locked file by others.
        let mut bytes = Vec::new();
        let info: Option<LockInfo> = std::io::Read::read_to_end(&mut f, &mut bytes)
            .ok()
            .and_then(|_| serde_json::from_slice(&bytes).ok());
        let _ = f.unlock();
        drop(f);
        let Some(info) = info.filter(|i| i.autosave.exists()) else {
            let _ = std::fs::remove_file(&path);
            continue;
        };
        let saved_at = std::fs::metadata(&info.autosave)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if found.as_ref().is_none_or(|r| saved_at > r.saved_at) {
            found = Some(Recovery {
                autosave: info.autosave,
                project: info.project,
                saved_at,
                lock: path,
            });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share the config folder through an environment variable.
    pub fn with_config_dir<R>(name: &str, f: impl FnOnce() -> R) -> R {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("reetched-config-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: tests that touch the variable hold LOCK.
        unsafe { std::env::set_var("REETCHED_CONFIG_DIR", &dir) };
        let r = f();
        unsafe { std::env::remove_var("REETCHED_CONFIG_DIR") };
        r
    }

    #[test]
    fn a_crashed_session_is_offered_back_and_a_clean_one_is_not() {
        with_config_dir("session", || {
            let mesh = Mesh::uv_sphere(1.0, 8, 4);
            let mut doc = Document::new(None, &["sphere".into()]);
            let resources = Resources::default();

            // A session that autosaves, then "crashes": its lock is released (as the
            // OS does for a dead process) but its files stay.
            let mut crashed = Session::start();
            crashed.last -= Duration::from_secs(3600);
            assert!(crashed.tick(1.0, &doc, &mesh, &resources, None).is_none());
            crashed.job.take().unwrap().join().unwrap().unwrap();
            assert!(crashed.autosave_path().exists());
            let lock = crashed.lock.take().unwrap();
            lock.unlock().unwrap();
            drop(lock);

            let next = Session::start();
            let r = next.recovery.as_ref().expect("crashed session found");
            let (back, _, _) = project::load(&r.autosave).unwrap();
            assert_eq!(back, doc);
            r.forget();

            // Nothing changed since: no new autosave.
            let mut s = next;
            s.mark_saved(&doc, None);
            s.last -= Duration::from_secs(3600);
            s.tick(1.0, &doc, &mesh, &resources, None);
            assert!(s.job.is_none());
            doc.texture_sets[0].resolution = 512;
            s.last -= Duration::from_secs(3600);
            s.tick(1.0, &doc, &mesh, &resources, None);
            assert!(s.job.is_some());
            s.end();
            assert!(
                Session::start().recovery.is_none(),
                "clean exit leaves nothing"
            );
        });
    }
}
