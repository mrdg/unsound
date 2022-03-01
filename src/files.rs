use anyhow::{anyhow, Result};
use camino::{Utf8Path, Utf8PathBuf};
use std::fs;
use std::path::PathBuf;

// TODO: handle empty directories better
pub struct FileBrowser {
    pub entries: Vec<Utf8PathBuf>,
    pub dir: Utf8PathBuf,
}

impl FileBrowser {
    pub fn with_path<P: AsRef<Utf8Path>>(path: P) -> Result<FileBrowser> {
        let mut fb = FileBrowser {
            entries: Vec::new(),
            dir: Utf8PathBuf::new(),
        };
        fb.move_to(path)?;
        Ok(fb)
    }

    pub fn move_to<P: AsRef<Utf8Path>>(&mut self, path: P) -> Result<()> {
        self.dir = utf8_path(path.as_ref().canonicalize()?)?;
        self.entries.clear();
        for entry in fs::read_dir(path.as_ref())? {
            let entry = entry?;
            if entry.path().is_dir() || entry.path().extension().map_or(false, |ext| ext == "wav") {
                let abs_path = utf8_path(entry.path().canonicalize()?)?;
                self.entries.push(abs_path);
            }
        }
        self.entries.sort();
        Ok(())
    }
}

fn utf8_path(path: PathBuf) -> Result<Utf8PathBuf> {
    Utf8PathBuf::from_path_buf(path).map_err(|path| anyhow!("invalid path {}", path.display()))
}
