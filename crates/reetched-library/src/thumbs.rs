//! Background thumbnail workers with an on-disk PNG cache.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::SystemTime;

use image::{DynamicImage, Rgba, RgbaImage, imageops};

use crate::{Asset, AssetId, AssetKind};

pub const DEFAULT_THUMB_SIZE: u32 = 128;
const WORKERS: usize = 2;
/// In-memory cap (~64 KB each at 128 px, so ~256 MB worst case); least recently
/// requested thumbnails are dropped first and reload from the disk cache.
const MAX_READY: usize = 4096;

/// The app's image decoder (SVG and anything else it knows). Return `None` to fall back
/// to the `image` crate.
pub type Decoder = Arc<dyn Fn(&Path, &[u8]) -> Option<RgbaImage> + Send + Sync>;

struct Job {
    id: AssetId,
    path: PathBuf,
    environment: bool,
}

enum Slot {
    /// Waiting; the highest `seq` (most recently requested) is decoded first.
    Queued(Job, u64),
    Working,
    /// `None` = decode failed; not retried until the file or the app changes.
    Ready(Option<Arc<RgbaImage>>, u64),
}

#[derive(Default)]
struct State {
    slots: HashMap<String, Slot>,
    seq: u64,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    cache_dir: PathBuf,
    size: u32,
    decoder: Option<Decoder>,
}

pub(crate) struct Thumbnails {
    shared: Arc<Shared>,
    pub ready: Receiver<AssetId>,
}

impl Thumbnails {
    pub fn new(cache_dir: PathBuf, size: u32, decoder: Option<Decoder>) -> Self {
        if let Err(e) = fs::create_dir_all(&cache_dir) {
            log::warn!("thumbnail cache {}: {e}", cache_dir.display());
        }
        let shared = Arc::new(Shared {
            state: Mutex::default(),
            wake: Condvar::new(),
            cache_dir,
            size,
            decoder,
        });
        let (tx, ready) = mpsc::channel();
        for _ in 0..WORKERS {
            let (shared, tx) = (shared.clone(), tx.clone());
            thread::Builder::new()
                .name("library-thumbs".into())
                .spawn(move || worker(&shared, &tx))
                .expect("spawning thumbnail thread");
        }
        Self { shared, ready }
    }

    pub fn get(&self, asset: &Asset) -> Option<Arc<RgbaImage>> {
        let key = cache_key(&asset.path, asset.size, asset.modified, self.shared.size);
        let mut st = self.shared.state.lock().unwrap();
        st.seq += 1;
        let seq = st.seq;
        match st.slots.get_mut(&key) {
            Some(Slot::Ready(img, s)) => {
                *s = seq;
                img.clone()
            }
            Some(Slot::Queued(_, s)) => {
                *s = seq;
                None
            }
            Some(Slot::Working) => None,
            None => {
                let job = Job {
                    id: asset.id.clone(),
                    path: asset.path.clone(),
                    environment: asset.kind == AssetKind::Environment,
                };
                st.slots.insert(key, Slot::Queued(job, seq));
                self.shared.wake.notify_one();
                None
            }
        }
    }
}

impl Drop for Thumbnails {
    fn drop(&mut self) {
        // Workers finish their current image and exit; not joined so the UI never waits
        // on a slow decode.
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.wake.notify_all();
    }
}

fn worker(shared: &Shared, tx: &Sender<AssetId>) {
    loop {
        let (key, job) = {
            let mut st = shared.state.lock().unwrap();
            loop {
                if st.shutdown {
                    return;
                }
                // ponytail: linear scan for the newest request; fine for thousands of queued
                // thumbnails, use a heap keyed by seq if queues get much larger.
                let next = st
                    .slots
                    .iter()
                    .filter_map(|(k, s)| {
                        if let Slot::Queued(_, seq) = s {
                            Some((*seq, k.clone()))
                        } else {
                            None
                        }
                    })
                    .max();
                if let Some((_, key)) = next {
                    let Some(Slot::Queued(job, _)) = st.slots.insert(key.clone(), Slot::Working)
                    else {
                        unreachable!()
                    };
                    break (key, job);
                }
                st = shared.wake.wait(st).unwrap();
            }
        };

        let file = shared.cache_dir.join(format!("{key}.png"));
        let img = image::open(&file).ok().map(|i| i.into_rgba8()).or_else(|| {
            let img = render(&job, shared.size, shared.decoder.as_ref())?;
            if let Err(e) = img.save_with_format(&file, image::ImageFormat::Png) {
                log::warn!("thumbnail cache write {}: {e}", file.display());
            }
            Some(img)
        });
        let ok = img.is_some();

        let mut st = shared.state.lock().unwrap();
        st.seq += 1;
        let seq = st.seq;
        st.slots.insert(key, Slot::Ready(img.map(Arc::new), seq));
        let ready: Vec<(u64, &String)> = st
            .slots
            .iter()
            .filter_map(|(k, s)| {
                if let Slot::Ready(_, seq) = s {
                    Some((*seq, k))
                } else {
                    None
                }
            })
            .collect();
        if ready.len() > MAX_READY {
            let oldest = ready.into_iter().min().map(|(_, k)| k.clone()).unwrap();
            st.slots.remove(&oldest);
        }
        drop(st);
        if ok && tx.send(job.id).is_err() {
            return;
        }
    }
}

/// Changes whenever the file, its size/time or the thumbnail size changes.
fn cache_key(path: &Path, size: u64, modified: SystemTime, thumb: u32) -> String {
    let nanos = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let id = format!("{}|{size}|{nanos}|{thumb}", path.display());
    blake3::hash(id.as_bytes()).to_hex()[..32].to_string()
}

fn render(job: &Job, size: u32, decoder: Option<&Decoder>) -> Option<RgbaImage> {
    let bytes = fs::read(&job.path).ok()?;
    if job.environment
        && let Some(img) = tonemapped(&bytes, size)
    {
        return Some(img);
    }
    let img = decoder
        .and_then(|d| d(&job.path, &bytes))
        .or_else(|| image::load_from_memory(&bytes).ok().map(|i| i.into_rgba8()))?;
    Some(fit(&img, size))
}

/// HDR/EXR -> Reinhard -> sRGB, keeping the lat-long 2:1 shape as a strip.
fn tonemapped(bytes: &[u8], size: u32) -> Option<RgbaImage> {
    let hdr = image::load_from_memory(bytes).ok()?.into_rgba32f();
    let (w, h) = fit_dims(hdr.width(), hdr.height(), size);
    let small = scaled(DynamicImage::ImageRgba32F(hdr), w, h).into_rgba32f();
    let encode = |v: f32| {
        let v = v.max(0.0);
        let t = v / (1.0 + v);
        let s = if t <= 0.003_130_8 {
            12.92 * t
        } else {
            1.055 * t.powf(1.0 / 2.4) - 0.055
        };
        (s * 255.0 + 0.5) as u8
    };
    let ldr = RgbaImage::from_fn(w, h, |x, y| {
        let p = small.get_pixel(x, y);
        Rgba([encode(p[0]), encode(p[1]), encode(p[2]), 255])
    });
    Some(fit(&ldr, size))
}

/// Scales to fit `size`×`size` keeping aspect, centered on a transparent square.
fn fit(img: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = fit_dims(img.width(), img.height(), size);
    let small = if (w, h) == img.dimensions() {
        img.clone()
    } else {
        scaled(DynamicImage::ImageRgba8(img.clone()), w, h).into_rgba8()
    };
    let mut canvas = RgbaImage::new(size, size);
    imageops::replace(
        &mut canvas,
        &small,
        ((size - w) / 2) as i64,
        ((size - h) / 2) as i64,
    );
    canvas
}

fn fit_dims(w: u32, h: u32, size: u32) -> (u32, u32) {
    let scale = size as f64 / w.max(h).max(1) as f64;
    let d = |v: u32| ((v as f64 * scale).round() as u32).clamp(1, size);
    (d(w), d(h))
}

/// Box-filtered `thumbnail` is far faster on 4K-8K sources; it can't enlarge, so small
/// images (alphas) go through `resize`.
fn scaled(img: DynamicImage, w: u32, h: u32) -> DynamicImage {
    if w <= img.width() && h <= img.height() {
        img.thumbnail_exact(w, h)
    } else {
        img.resize_exact(w, h, imageops::FilterType::Triangle)
    }
}
