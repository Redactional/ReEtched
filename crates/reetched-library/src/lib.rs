//! Asset library (the shelf): folders of materials, smart materials, masks, brushes,
//! alphas, textures, environments and shaders, with tags, favorites and search,
//! kept up to date when files change on disk.
//!
//! Threading:
//! - The UI thread owns [`Library`] and calls [`Library::poll`] once per frame. `poll`
//!   applies finished folder scans to the asset list and returns [`LibraryEvent`]s.
//! - One scanner thread receives file-system notifications (from `notify`'s own thread),
//!   debounces them (~300 ms) and walks only the changed paths. Roots that can't be
//!   watched are re-walked every few seconds instead and diffed by size + modified time.
//! - Two thumbnail threads decode images (see [`Library::with_thumbnails`]).
//!
//! [`Library::add_root`], [`Library::rescan`], [`Library::import_into`] and
//! [`Library::remove`] do their file work synchronously on the calling thread.

mod scan;
mod thumbs;

pub use thumbs::{DEFAULT_THUMB_SIZE, Decoder};

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use image::RgbaImage;
use serde::{Deserialize, Serialize};

use scan::FileInfo;

/// Per-root metadata file, written at the root of writable libraries so tags travel
/// with a shared folder.
pub const METADATA_FILE: &str = ".reetched-library.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    Texture,
    Alpha,
    Environment,
    SmartMaterial,
    SmartMask,
    Brush,
    Procedural,
    Shader,
    OcioConfig,
    ProjectTemplate,
    Mesh,
}

impl AssetKind {
    pub const ALL: [AssetKind; 11] = [
        Self::Texture,
        Self::Alpha,
        Self::Environment,
        Self::SmartMaterial,
        Self::SmartMask,
        Self::Brush,
        Self::Procedural,
        Self::Shader,
        Self::OcioConfig,
        Self::ProjectTemplate,
        Self::Mesh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Texture => "Texture",
            Self::Alpha => "Alpha",
            Self::Environment => "Environment",
            Self::SmartMaterial => "Smart material",
            Self::SmartMask => "Smart mask",
            Self::Brush => "Brush",
            Self::Procedural => "Procedural",
            Self::Shader => "Shader",
            Self::OcioConfig => "OCIO config",
            Self::ProjectTemplate => "Project template",
            Self::Mesh => "Mesh",
        }
    }

    /// Subfolder [`Library::import_into`] copies this kind into.
    fn folder(self) -> &'static str {
        match self {
            Self::Texture => "textures",
            Self::Alpha => "alphas",
            Self::Environment => "environments",
            Self::SmartMaterial => "smart-materials",
            Self::SmartMask => "smart-masks",
            Self::Brush => "brushes",
            Self::Procedural => "procedurals",
            Self::Shader => "shaders",
            Self::OcioConfig => "ocio",
            Self::ProjectTemplate => "templates",
            Self::Mesh => "meshes",
        }
    }

    /// Parses the `kind:` search keyword; ignores case, spaces, `-` and `_`.
    pub fn parse(s: &str) -> Option<Self> {
        let s: String = s
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase();
        Some(match s.as_str() {
            "texture" | "textures" => Self::Texture,
            "alpha" | "alphas" => Self::Alpha,
            "environment" | "environments" | "env" | "hdri" => Self::Environment,
            "smartmaterial" | "smartmaterials" => Self::SmartMaterial,
            "smartmask" | "smartmasks" => Self::SmartMask,
            "brush" | "brushes" => Self::Brush,
            "procedural" | "procedurals" | "sbs" => Self::Procedural,
            "shader" | "shaders" => Self::Shader,
            "ocio" | "ocioconfig" => Self::OcioConfig,
            "template" | "projecttemplate" | "templates" => Self::ProjectTemplate,
            "mesh" | "meshes" => Self::Mesh,
            _ => return None,
        })
    }

    /// Kinds the thumbnail worker renders; the UI draws an icon for the rest.
    pub fn has_thumbnail(self) -> bool {
        matches!(self, Self::Texture | Self::Alpha | Self::Environment)
    }
}

/// Classifies a path relative to its library root, by extension and parent folder names.
pub fn classify(rel: &Path) -> Option<AssetKind> {
    use AssetKind::*;
    let ext = rel.extension()?.to_str()?.to_ascii_lowercase();
    let dirs: Vec<String> = rel
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| c.as_os_str().to_str())
                .map(str::to_lowercase)
                .collect()
        })
        .unwrap_or_default();
    let in_dir = |names: &[&str]| dirs.iter().any(|d| names.contains(&d.as_str()));
    let alpha = in_dir(&["alpha", "alphas", "brushes alphas"]);
    let env = in_dir(&["environment", "environments", "hdri", "hdris", "env"]);
    Some(match ext.as_str() {
        "hdr" => Environment,
        "exr" if env && !alpha => Environment,
        "png" | "jpg" | "jpeg" | "tga" | "tif" | "tiff" | "exr" | "bmp" | "webp" | "svg" => {
            if alpha { Alpha } else { Texture }
        }
        "rsm" => SmartMaterial,
        "rsmk" => SmartMask,
        "rbrush" => Brush,
        "sbs" => Procedural,
        "glsl" => Shader,
        "ocio" => OcioConfig,
        "rtpl" => ProjectTemplate,
        "fbx" | "obj" | "gltf" | "glb" | "usd" | "usda" | "usdc" | "usdz" => Mesh,
        _ => return None,
    })
}

/// "rusty_metal-01" -> "Rusty Metal 01".
pub fn pretty_name(stem: &str) -> String {
    stem.split(|c: char| c == '_' || c == '-' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Stable id: `"<root name>/<relative path with forward slashes>"`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetId(pub String);

impl AssetId {
    fn new(root: &str, rel: &str) -> Self {
        Self(format!("{root}/{rel}"))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub id: AssetId,
    /// Name of the [`LibraryRoot`] this asset lives in.
    pub root: String,
    /// Path relative to the root, `/`-separated.
    pub rel_path: String,
    pub kind: AssetKind,
    /// User-set display name, else the prettified file stem.
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
    pub tags: Vec<String>,
    pub favorite: bool,
}

#[derive(Clone, Debug)]
pub struct LibraryRoot {
    /// Unique, shown in the UI and used in asset ids. Must not contain `/`.
    pub name: String,
    pub path: PathBuf,
    /// The user's own folder: imports go here and metadata is stored inside it.
    /// Read-only roots keep their metadata in the library's metadata dir.
    pub writable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryEvent {
    Added(AssetId),
    Removed(AssetId),
    /// File content, tags, favorite or display name changed.
    Changed(AssetId),
    /// A thumbnail finished; the next [`Library::thumbnail`] call returns it.
    ThumbnailReady(AssetId),
    /// The OS watcher couldn't watch this root (e.g. network drive); it is re-scanned
    /// periodically instead.
    WatchFailed {
        root: String,
        error: String,
    },
}

/// On-disk metadata, one file per root. Keys are `/`-separated paths relative to the root.
#[derive(Default, Serialize, Deserialize)]
struct MetaFile {
    version: u32,
    assets: BTreeMap<String, MetaEntry>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct MetaEntry {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Size and fast hash identify the file again after it moves within the root.
    #[serde(default)]
    size: u64,
    #[serde(default)]
    hash: String,
}

impl MetaEntry {
    fn is_empty(&self) -> bool {
        self.tags.is_empty() && !self.favorite && self.name.is_none()
    }
}

struct Root {
    info: LibraryRoot,
    meta: MetaFile,
    meta_path: PathBuf,
}

pub struct Library {
    roots: Vec<Root>,
    assets: BTreeMap<AssetId, Asset>,
    meta_dir: PathBuf,
    pending: Vec<LibraryEvent>,
    scanner: scan::Scanner,
    thumbs: Option<thumbs::Thumbnails>,
}

impl Library {
    /// `meta_dir` holds metadata of read-only roots (a per-user config dir).
    /// Starts the scanner thread and the OS file watcher.
    pub fn new(meta_dir: impl Into<PathBuf>) -> Self {
        Self {
            roots: Vec::new(),
            assets: BTreeMap::new(),
            meta_dir: meta_dir.into(),
            pending: Vec::new(),
            scanner: scan::Scanner::new(),
            thumbs: None,
        }
    }

    /// Enables thumbnails: two worker threads, PNG cache in `cache_dir`, `size` px square
    /// (use [`DEFAULT_THUMB_SIZE`]). `decoder` is tried first for every image; the `image`
    /// crate is the fallback when it returns `None`.
    pub fn with_thumbnails(
        mut self,
        cache_dir: impl Into<PathBuf>,
        size: u32,
        decoder: Option<Decoder>,
    ) -> Self {
        self.thumbs = Some(thumbs::Thumbnails::new(
            cache_dir.into(),
            size.max(1),
            decoder,
        ));
        self
    }

    /// Adds a root and scans it synchronously (no events for the initial contents).
    pub fn add_root(&mut self, root: LibraryRoot) -> Result<()> {
        if root.name.is_empty() || root.name.contains('/') {
            bail!("invalid library name {:?}", root.name);
        }
        if self.roots.iter().any(|r| r.info.name == root.name) {
            bail!("a library named {:?} already exists", root.name);
        }
        let path = std::path::absolute(&root.path)?;
        let meta_path = if root.writable {
            path.join(METADATA_FILE)
        } else {
            self.meta_dir.join(format!("{}.json", sanitize(&root.name)))
        };
        let meta = load_meta(&meta_path);
        let name = root.name.clone();
        self.roots.push(Root {
            info: LibraryRoot {
                path: path.clone(),
                ..root
            },
            meta,
            meta_path,
        });

        let before = self.pending.len();
        self.apply_scan(&name, "", scan::walk(&path, &path));
        self.pending.truncate(before);

        if let Err(error) = self.scanner.add_root(&name, &path) {
            log::warn!("library {name:?}: watching failed ({error}), polling instead");
            self.pending
                .push(LibraryEvent::WatchFailed { root: name, error });
        }
        Ok(())
    }

    pub fn remove_root(&mut self, name: &str) {
        let Some(i) = self.root_index(name) else {
            return;
        };
        let root = self.roots.remove(i);
        self.scanner.remove_root(name, &root.info.path);
        self.assets.retain(|_, a| a.root != name);
    }

    pub fn roots(&self) -> impl Iterator<Item = &LibraryRoot> {
        self.roots.iter().map(|r| &r.info)
    }

    /// All assets, ordered by id.
    pub fn assets(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }

    pub fn get(&self, id: &AssetId) -> Option<&Asset> {
        self.assets.get(id)
    }

    /// Every tag in use, sorted.
    pub fn all_tags(&self) -> Vec<String> {
        self.assets
            .values()
            .flat_map(|a| a.tags.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Call once per frame: applies finished background scans and returns what changed.
    pub fn poll(&mut self) -> Vec<LibraryEvent> {
        while let Ok(r) = self.scanner.results.try_recv() {
            self.apply_scan(&r.root, &r.prefix, r.files);
        }
        if let Some(t) = &self.thumbs {
            while let Ok(id) = t.ready.try_recv() {
                self.pending.push(LibraryEvent::ThumbnailReady(id));
            }
        }
        std::mem::take(&mut self.pending)
    }

    /// Full synchronous re-walk of every root ("refresh" button). Events go to `poll`.
    pub fn rescan(&mut self) {
        let roots: Vec<(String, PathBuf)> = self
            .roots
            .iter()
            .map(|r| (r.info.name.clone(), r.info.path.clone()))
            .collect();
        for (name, path) in roots {
            self.apply_scan(&name, "", scan::walk(&path, &path));
        }
    }

    /// Search. `text` holds space-separated terms that must all match:
    /// - `word` / `"quoted phrase"`: substring of the name or of a tag (case-insensitive);
    /// - `tag:metal` / `tag:"rusty metal"`: substring of a tag;
    /// - `kind:alpha`: asset kind (see [`AssetKind::parse`]).
    ///
    /// Sorted by relevance (exact > prefix > word start > substring; name before tag), then name.
    pub fn query(&self, text: &str, kind: Option<AssetKind>, favorites_only: bool) -> Vec<&Asset> {
        let q = Query::parse(text);
        let mut hits: Vec<(u32, String, &Asset)> = self
            .assets
            .values()
            .filter(|a| kind.is_none_or(|k| a.kind == k) && (!favorites_only || a.favorite))
            .filter_map(|a| q.score(a).map(|s| (s, a.name.to_lowercase(), a)))
            .collect();
        hits.sort_by(|x, y| {
            y.0.cmp(&x.0)
                .then_with(|| x.1.cmp(&y.1))
                .then_with(|| x.2.id.cmp(&y.2.id))
        });
        hits.into_iter().map(|h| h.2).collect()
    }

    /// Replaces the tags (trimmed, de-duplicated ignoring case, empty ones dropped).
    pub fn set_tags<S: Into<String>>(
        &mut self,
        id: &AssetId,
        tags: impl IntoIterator<Item = S>,
    ) -> Result<()> {
        let mut clean: Vec<String> = Vec::new();
        for t in tags {
            let t = t.into().trim().to_string();
            if !t.is_empty() && !clean.iter().any(|c| c.eq_ignore_ascii_case(&t)) {
                clean.push(t);
            }
        }
        self.edit_meta(id, |e| e.tags = clean)
    }

    pub fn set_favorite(&mut self, id: &AssetId, favorite: bool) -> Result<()> {
        self.edit_meta(id, |e| e.favorite = favorite)
    }

    /// Overrides the display name; `None` (or blank) goes back to the file name.
    pub fn set_display_name(&mut self, id: &AssetId, name: Option<String>) -> Result<()> {
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        self.edit_meta(id, |e| e.name = name)
    }

    /// Copies `files` into the writable root `root`, each into a subfolder for its kind
    /// (`textures/`, `brushes/`, ...), renaming on collision. Returns the new ids.
    pub fn import_into(&mut self, root: &str, files: &[impl AsRef<Path>]) -> Result<Vec<AssetId>> {
        let ri = self
            .root_index(root)
            .with_context(|| format!("no library named {root:?}"))?;
        if !self.roots[ri].info.writable {
            bail!("library {root:?} is read-only");
        }
        // Check every file first so a bad one doesn't leave a half-done import.
        let mut plan = Vec::new();
        for f in files {
            let src = f.as_ref();
            let file_name = src
                .file_name()
                .with_context(|| format!("not a file: {}", src.display()))?;
            let kind = classify(Path::new(file_name))
                .with_context(|| format!("unsupported asset type: {}", src.display()))?;
            plan.push((src, file_name, kind));
        }
        // ponytail: classified by file name alone, so images always land in textures/;
        // add an explicit kind argument when the UI needs to import alphas directly.
        let root_path = self.roots[ri].info.path.clone();
        let mut ids = Vec::new();
        for (src, file_name, kind) in plan {
            let dir = root_path.join(kind.folder());
            fs::create_dir_all(&dir)?;
            let dest = unique_path(dir.join(file_name));
            fs::copy(src, &dest).with_context(|| format!("copying {}", src.display()))?;
            let rel = scan::rel_string(&root_path, &dest).context("bad destination path")?;
            self.apply_scan(root, &rel, scan::walk(&root_path, &dest));
            ids.push(AssetId::new(root, &rel));
        }
        Ok(ids)
    }

    /// Permanently deletes the asset's file and its metadata (no OS trash).
    /// The caller must have asked the user to confirm first.
    pub fn remove(&mut self, id: &AssetId) -> Result<()> {
        let asset = self
            .assets
            .get(id)
            .with_context(|| format!("unknown asset {id}"))?;
        let (root, rel, path) = (
            asset.root.clone(),
            asset.rel_path.clone(),
            asset.path.clone(),
        );
        fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))?;
        let ri = self.root_index(&root).context("asset root missing")?;
        if self.roots[ri].meta.assets.remove(&rel).is_some() {
            self.save_meta(ri);
        }
        self.apply_scan(&root, &rel, Vec::new());
        Ok(())
    }

    /// The thumbnail if ready; otherwise queues it (most recently requested first) and
    /// returns `None`. Always `None` for kinds without thumbnails or a failed decode.
    pub fn thumbnail(&self, id: &AssetId) -> Option<Arc<RgbaImage>> {
        let asset = self.assets.get(id)?;
        if !asset.kind.has_thumbnail() {
            return None;
        }
        self.thumbs.as_ref()?.get(asset)
    }

    fn root_index(&self, name: &str) -> Option<usize> {
        self.roots.iter().position(|r| r.info.name == name)
    }

    /// Reconciles assets at or under `prefix` ("" = whole root) with a fresh listing.
    fn apply_scan(&mut self, root: &str, prefix: &str, files: Vec<FileInfo>) {
        let Some(ri) = self.root_index(root) else {
            return;
        };
        let under = |rel: &str| {
            prefix.is_empty()
                || rel == prefix
                || (rel.starts_with(prefix) && rel[prefix.len()..].starts_with('/'))
        };
        let found: HashMap<String, FileInfo> = files
            .into_iter()
            .filter(|f| under(&f.rel) && classify(Path::new(&f.rel)).is_some())
            .map(|f| (f.rel.clone(), f))
            .collect();

        let root_prefix = format!("{root}/");
        let gone: Vec<AssetId> = self
            .assets
            .range(AssetId(root_prefix.clone())..)
            .take_while(|(id, _)| id.0.starts_with(&root_prefix))
            .filter(|(_, a)| under(&a.rel_path) && !found.contains_key(&a.rel_path))
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            self.assets.remove(&id);
            self.pending.push(LibraryEvent::Removed(id));
        }

        let mut meta_dirty = false;
        for (rel, f) in found {
            let id = AssetId::new(root, &rel);
            if let Some(a) = self.assets.get_mut(&id) {
                if a.size != f.size || a.modified != f.modified {
                    a.size = f.size;
                    a.modified = f.modified;
                    // Keep the stored hash current so a later move is still recognised.
                    if let Some(e) = self.roots[ri].meta.assets.get_mut(&rel) {
                        e.size = f.size;
                        e.hash = fast_hash(&f.path).unwrap_or_default();
                        meta_dirty = true;
                    }
                    self.pending.push(LibraryEvent::Changed(id));
                }
                continue;
            }
            if !self.roots[ri].meta.assets.contains_key(&rel)
                && let Some(old) = self.find_moved(ri, &f)
            {
                let meta = &mut self.roots[ri].meta.assets;
                let entry = meta.remove(&old).expect("candidate came from the map");
                meta.insert(rel.clone(), entry);
                meta_dirty = true;
            }
            let entry = self.roots[ri].meta.assets.get(&rel);
            let stem = Path::new(&rel)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let asset = Asset {
                id: id.clone(),
                root: root.to_string(),
                kind: classify(Path::new(&rel)).expect("filtered above"),
                name: entry
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| pretty_name(stem)),
                path: f.path,
                size: f.size,
                modified: f.modified,
                tags: entry.map(|e| e.tags.clone()).unwrap_or_default(),
                favorite: entry.is_some_and(|e| e.favorite),
                rel_path: rel,
            };
            self.assets.insert(id.clone(), asset);
            self.pending.push(LibraryEvent::Added(id));
        }
        if meta_dirty {
            self.save_meta(ri);
        }
    }

    /// A metadata entry whose file is gone and whose size + hash match `f`: the file moved.
    /// Hashes only when a same-size orphan exists, so normal scans read no file content.
    fn find_moved(&self, ri: usize, f: &FileInfo) -> Option<String> {
        let root = &self.roots[ri];
        let candidates: Vec<&String> = root
            .meta
            .assets
            .iter()
            .filter(|(rel, e)| {
                e.size == f.size && !e.hash.is_empty() && !root.info.path.join(rel).exists()
            })
            .map(|(rel, _)| rel)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let hash = fast_hash(&f.path).ok()?;
        candidates
            .into_iter()
            .find(|rel| root.meta.assets[*rel].hash == hash)
            .cloned()
    }

    fn edit_meta(&mut self, id: &AssetId, edit: impl FnOnce(&mut MetaEntry)) -> Result<()> {
        let asset = self
            .assets
            .get_mut(id)
            .with_context(|| format!("unknown asset {id}"))?;
        let ri = self
            .roots
            .iter()
            .position(|r| r.info.name == asset.root)
            .context("asset root missing")?;
        let meta = &mut self.roots[ri].meta.assets;
        let entry = meta.entry(asset.rel_path.clone()).or_default();
        edit(entry);
        entry.size = asset.size;
        entry.hash = fast_hash(&asset.path).unwrap_or_default();
        asset.tags = entry.tags.clone();
        asset.favorite = entry.favorite;
        asset.name = match &entry.name {
            Some(n) => n.clone(),
            None => pretty_name(
                Path::new(&asset.rel_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
            ),
        };
        if entry.is_empty() {
            meta.remove(&asset.rel_path);
        }
        self.pending.push(LibraryEvent::Changed(id.clone()));
        self.save_meta(ri);
        Ok(())
    }

    fn save_meta(&mut self, ri: usize) {
        let root = &mut self.roots[ri];
        root.meta.version = 1;
        let write = || -> Result<()> {
            if let Some(dir) = root.meta_path.parent() {
                fs::create_dir_all(dir)?;
            }
            // Write-then-rename so a crash never leaves a truncated file.
            let tmp = root.meta_path.with_extension("json.tmp");
            fs::write(&tmp, serde_json::to_vec_pretty(&root.meta)?)?;
            fs::rename(&tmp, &root.meta_path)?;
            Ok(())
        };
        if let Err(e) = write() {
            log::error!(
                "saving library metadata {}: {e:#}",
                root.meta_path.display()
            );
        }
    }
}

fn load_meta(path: &Path) -> MetaFile {
    let Ok(bytes) = fs::read(path) else {
        return MetaFile::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(m) => m,
        Err(e) => {
            // Keep the unreadable file aside rather than overwriting the user's tags.
            let backup = path.with_extension("json.corrupt");
            log::error!(
                "library metadata {} unreadable ({e}); moved to {}",
                path.display(),
                backup.display()
            );
            let _ = fs::rename(path, backup);
            MetaFile::default()
        }
    }
}

/// blake3 of the first and last 64 KB plus the length: cheap even for 8K EXRs.
fn fast_hash(path: &Path) -> std::io::Result<String> {
    const CHUNK: u64 = 64 * 1024;
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    let mut buf = Vec::new();
    (&mut file).take(CHUNK).read_to_end(&mut buf)?;
    if len > CHUNK {
        file.seek(SeekFrom::Start(len.saturating_sub(CHUNK).max(CHUNK)))?;
        file.take(CHUNK).read_to_end(&mut buf)?;
    }
    hasher.update(&buf);
    hasher.update(&len.to_le_bytes());
    Ok(hasher.finalize().to_hex()[..32].to_string())
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// `foo.png` -> `foo (2).png` -> `foo (3).png` ... until free.
fn unique_path(path: PathBuf) -> PathBuf {
    if !path.exists() {
        return path;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("asset")
        .to_string();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

struct Query {
    terms: Vec<String>,
    tags: Vec<String>,
    /// `None` = unknown kind keyword, which matches nothing.
    kinds: Vec<Option<AssetKind>>,
}

impl Query {
    fn parse(text: &str) -> Self {
        let mut tokens = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        for c in text.chars() {
            match c {
                '"' => quoted = !quoted,
                c if c.is_whitespace() && !quoted => {
                    if !cur.is_empty() {
                        tokens.push(std::mem::take(&mut cur));
                    }
                }
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            tokens.push(cur);
        }
        let mut q = Query {
            terms: Vec::new(),
            tags: Vec::new(),
            kinds: Vec::new(),
        };
        for t in tokens {
            let t = t.to_lowercase();
            if let Some(tag) = t.strip_prefix("tag:") {
                if !tag.is_empty() {
                    q.tags.push(tag.to_string());
                }
            } else if let Some(kind) = t.strip_prefix("kind:") {
                q.kinds.push(AssetKind::parse(kind));
            } else {
                q.terms.push(t);
            }
        }
        q
    }

    fn score(&self, a: &Asset) -> Option<u32> {
        if !self.kinds.iter().all(|k| *k == Some(a.kind)) {
            return None;
        }
        let name = a.name.to_lowercase();
        let tags: Vec<String> = a.tags.iter().map(|t| t.to_lowercase()).collect();
        let tag_score = |term: &str| {
            tags.iter()
                .map(|t| {
                    if t == term {
                        30
                    } else if t.contains(term) {
                        10
                    } else {
                        0
                    }
                })
                .max()
                .unwrap_or(0)
        };
        let mut total = 0;
        for tag in &self.tags {
            match tag_score(tag) {
                0 => return None,
                s => total += s,
            }
        }
        for term in &self.terms {
            let name_score = if name == *term {
                100
            } else if name.starts_with(term.as_str()) {
                60
            } else if name
                .split(|c: char| !c.is_alphanumeric())
                .any(|w| w.starts_with(term.as_str()))
            {
                40
            } else if name.contains(term.as_str()) {
                20
            } else {
                0
            };
            match name_score.max(tag_score(term)) {
                0 => return None,
                s => total += s,
            }
        }
        Some(total)
    }
}
