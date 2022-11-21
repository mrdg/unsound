use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use std::convert::TryInto;
use std::fs;

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
        self.dir = path.as_ref().canonicalize()?.try_into()?;
        self.entries.clear();
        for entry in fs::read_dir(path.as_ref())? {
            let entry = entry?;
            let path = entry.path().canonicalize()?.try_into()?;
            self.entries.push(path);
        }
        self.entries.sort();
        Ok(())
    }
}
