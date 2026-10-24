//! The driver's compiled-pipeline cache, kept on disk between runs so shaders
//! don't recompile on every start (Vulkan only; elsewhere this does nothing).

use std::{path::PathBuf, sync::OnceLock};

struct Cache {
    cache: wgpu::PipelineCache,
    path: PathBuf,
}

static CACHE: OnceLock<Option<Cache>> = OnceLock::new();

/// Sets the cache up for `device` (call before creating pipelines).
pub fn init(device: &wgpu::Device, adapter: &wgpu::AdapterInfo) {
    CACHE.get_or_init(|| {
        if !device.features().contains(wgpu::Features::PIPELINE_CACHE) {
            return None;
        }
        let key = wgpu::util::pipeline_cache_key(adapter)?;
        let path = crate::settings::config_dir().join("cache").join(key);
        let data = std::fs::read(&path).ok();
        // SAFETY: the data was written by `save` from `PipelineCache::get_data`, for
        // this adapter (the file name is the driver's cache key); `fallback` makes
        // the driver ignore data it doesn't accept.
        let cache = unsafe {
            device.create_pipeline_cache(&wgpu::PipelineCacheDescriptor {
                label: Some("reetched pipelines"),
                data: data.as_deref(),
                fallback: true,
            })
        };
        Some(Cache { cache, path })
    });
}

/// The cache to pass when creating pipelines (`None` without one).
pub fn get() -> Option<&'static wgpu::PipelineCache> {
    CACHE.get()?.as_ref().map(|c| &c.cache)
}

/// Writes the cache to disk (at exit).
pub fn save() {
    let Some(Some(c)) = CACHE.get() else { return };
    let Some(data) = c.cache.get_data() else {
        return;
    };
    if let Some(dir) = c.path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = c.path.with_extension("tmp");
    if std::fs::write(&tmp, data).is_ok() {
        let _ = std::fs::rename(&tmp, &c.path);
    }
}
