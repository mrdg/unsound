use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use std::convert::TryInto;
use std::fs::{self, FileType};

pub struct Entry {
    pub path: Utf8PathBuf,
    pub file_type: FileType,
}

// TODO: handle empty directories better
pub struct FileBrowser {
    pub entries: Vec<Entry>,
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
            if let Ok(path) = entry.path().canonicalize() {
                self.entries.push(Entry {
                    path: path.try_into()?,
                    file_type: entry.file_type()?,
                });
            } else {
                continue;
            }
        }
        self.entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(())
    }
}
