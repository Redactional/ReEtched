//! The `.spp` HDF5 container: named byte datasets in groups.

use anyhow::{Context, Result};
use std::path::Path;

pub struct Container {
    file: hdf5_pure::File,
}

impl Container {
    /// Opens lazily: datasets are read when asked for (projects can be gigabytes).
    pub fn open(path: &Path) -> Result<Self> {
        let file = hdf5_pure::File::open_streaming(path)
            .with_context(|| format!("{} is not a Painter project (HDF5)", path.display()))?;
        Ok(Self { file })
    }

    /// The bytes of dataset `path` ("paint/document.bin").
    pub fn bytes(&self, path: &str) -> Result<Vec<u8>> {
        let ds = self
            .file
            .dataset(path)
            .with_context(|| format!("missing {path}"))?;
        ds.read::<u8>().with_context(|| format!("reading {path}"))
    }

    pub fn has(&self, path: &str) -> bool {
        self.file.dataset(path).is_ok()
    }

    /// Dataset names directly inside group `path` ("" = root).
    pub fn datasets(&self, path: &str) -> Result<Vec<String>> {
        Ok(self.group(path)?.datasets()?)
    }

    /// Group names directly inside group `path`.
    pub fn groups(&self, path: &str) -> Result<Vec<String>> {
        Ok(self.group(path)?.groups()?)
    }

    fn group(&self, path: &str) -> Result<hdf5_pure::Group> {
        if path.is_empty() {
            Ok(self.file.root())
        } else {
            self.file
                .group(path)
                .with_context(|| format!("missing group {path}"))
        }
    }
}
