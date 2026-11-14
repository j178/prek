use std::io::{self, Cursor, Read};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use anyhow::Context;
use rustc_hash::FxHashMap;

const MAX_ENTRIES: usize = 512;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_FILE_BYTES: usize = 256 * 1024;

/// Shares reads within a workspace-read-only hook group. Disabled by default.
/// Entries stay until the group ends so a faster hook cannot evict a slower hook's inputs.
#[derive(Clone, Default)]
pub(crate) struct FileReadCache {
    state: Option<Arc<Mutex<State>>>,
}

#[derive(Default)]
struct State {
    entries: FxHashMap<PathBuf, Arc<Mutex<Entry>>>,
    bytes: usize,
}

#[derive(Default)]
enum Entry {
    #[default]
    Empty,
    Cached(Arc<[u8]>),
    Bypass,
}

impl FileReadCache {
    pub(crate) fn new() -> Self {
        Self {
            state: Some(Arc::default()),
        }
    }

    pub(crate) fn read(&self, path: &Path) -> io::Result<FileContents> {
        let Some(state) = &self.state else {
            return fs_err::read(path).map(FileContents::Owned);
        };
        let entry = {
            let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(entry) = state.entries.get(path) {
                Arc::clone(entry)
            } else if state.entries.len() < MAX_ENTRIES && state.bytes < MAX_BYTES {
                let entry = Arc::default();
                state.entries.insert(path.to_path_buf(), Arc::clone(&entry));
                entry
            } else {
                drop(state);
                return fs_err::read(path).map(FileContents::Owned);
            }
        };

        // Readers of the same file wait for its first load, without blocking other paths.
        let mut entry = entry.lock().unwrap_or_else(PoisonError::into_inner);
        match &*entry {
            Entry::Cached(content) => {
                return Ok(FileContents::Cached(Arc::clone(content)));
            }
            Entry::Bypass => {
                drop(entry);
                return fs_err::read(path).map(FileContents::Owned);
            }
            Entry::Empty => {}
        }

        let mut file = fs_err::File::open(path)?;
        let metadata = file.metadata().ok();
        let size = metadata
            .as_ref()
            .and_then(|metadata| usize::try_from(metadata.len()).ok())
            .unwrap_or(0);
        if !metadata.is_some_and(|metadata| metadata.is_file()) || size > MAX_FILE_BYTES {
            *entry = Entry::Bypass;
            drop(entry);
            let mut content = Vec::with_capacity(size);
            file.read_to_end(&mut content)?;
            return Ok(FileContents::Owned(content));
        }

        let mut content = Vec::with_capacity(size);
        file.read_to_end(&mut content)?;
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        if content.len() > MAX_FILE_BYTES || content.len() > MAX_BYTES - state.bytes {
            *entry = Entry::Bypass;
            return Ok(FileContents::Owned(content));
        }
        state.bytes += content.len();
        drop(state);

        let content: Arc<[u8]> = content.into();
        *entry = Entry::Cached(Arc::clone(&content));
        Ok(FileContents::Cached(content))
    }

    /// Streaming and prefix checks only consume existing entries, preserving early exits.
    pub(crate) fn open(&self, path: &Path) -> io::Result<FileReader> {
        if let Some(state) = &self.state {
            let entry = state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entries
                .get(path)
                .cloned();
            if let Some(entry) = entry {
                let entry = entry.lock().unwrap_or_else(PoisonError::into_inner);
                if let Entry::Cached(content) = &*entry {
                    return Ok(FileReader::Cached(Cursor::new(Arc::clone(content))));
                }
            }
        }
        fs_err::File::open(path).map(FileReader::File)
    }
}

pub(crate) enum FileContents {
    Owned(Vec<u8>),
    Cached(Arc<[u8]>),
}

impl FileContents {
    pub(crate) fn as_str(&self, path: &Path) -> anyhow::Result<&str> {
        std::str::from_utf8(self)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "stream did not contain valid UTF-8",
                )
            })
            .with_context(|| format!("failed to read from file `{}`", path.display()))
    }
}

impl Deref for FileContents {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(content) => content,
            Self::Cached(content) => content,
        }
    }
}

pub(crate) enum FileReader {
    File(fs_err::File),
    Cached(Cursor<Arc<[u8]>>),
}

impl Read for FileReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::File(file) => file.read(buffer),
            Self::Cached(content) => content.read(buffer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};
    use std::sync::Barrier;

    /// Concurrent clones share successful reads; a missing file can be retried after creation.
    #[test]
    fn shared_reads_and_retry_after_failure() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("contents");
        let cache = FileReadCache::new();
        assert_eq!(
            cache.read(&path).err().context("missing file")?.kind(),
            io::ErrorKind::NotFound
        );
        fs_err::write(&path, b"contents\xff")?;

        let barrier = Barrier::new(8);
        std::thread::scope(|scope| -> Result<()> {
            let readers: Vec<_> = (0..8)
                .map(|_| {
                    let cache = cache.clone();
                    let path = &path;
                    let barrier = &barrier;
                    scope.spawn(move || -> Result<()> {
                        barrier.wait();
                        assert_eq!(&*cache.read(path)?, b"contents\xff");
                        Ok(())
                    })
                })
                .collect();
            for reader in readers {
                reader
                    .join()
                    .map_err(|_| anyhow::anyhow!("cache reader panicked"))??;
            }
            Ok(())
        })?;

        // Removing the source makes any accidental second read observable.
        fs_err::remove_file(&path)?;
        assert_eq!(&*cache.clone().read(&path)?, b"contents\xff");
        let mut content = Vec::new();
        cache.open(&path)?.read_to_end(&mut content)?;
        assert_eq!(content, b"contents\xff");
        assert!(FileReadCache::new().read(&path).is_err());
        Ok(())
    }

    /// A prefix read never populates a partial snapshot, and large files remain uncached.
    #[test]
    fn streaming_misses_and_large_files_remain_live() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("contents");
        let cache = FileReadCache::new();
        fs_err::write(&path, b"#!old")?;
        let mut prefix = [0; 2];
        cache.open(&path)?.read_exact(&mut prefix)?;
        assert_eq!(prefix, *b"#!");
        fs_err::write(&path, b"#!new")?;
        assert_eq!(&*cache.read(&path)?, b"#!new");

        let large = dir.path().join("large");
        let content = vec![b'x'; MAX_FILE_BYTES + 1];
        fs_err::write(&large, &content)?;
        assert_eq!(&*cache.read(&large)?, content);
        fs_err::write(&large, b"updated")?;
        assert_eq!(&*cache.read(&large)?, b"updated");
        Ok(())
    }

    /// Memory stays bounded by both bytes and entry count, including empty files.
    #[test]
    fn retention_is_bounded() -> Result<()> {
        for (count, size) in [
            (MAX_ENTRIES + 1, 0),
            (MAX_BYTES / MAX_FILE_BYTES + 1, MAX_FILE_BYTES),
        ] {
            let dir = tempfile::tempdir()?;
            let cache = FileReadCache::new();
            let contents = vec![b'x'; size];
            for index in 0..count {
                let path = dir.path().join(index.to_string());
                fs_err::write(&path, &contents)?;
                assert_eq!(&*cache.read(&path)?, contents);
                fs_err::remove_file(&path)?;
            }
            let mut retained_entries = 0;
            let mut retained_bytes = 0;
            for index in 0..count {
                if let Ok(content) = cache.read(&dir.path().join(index.to_string())) {
                    retained_entries += 1;
                    retained_bytes += content.len();
                }
            }
            assert!(retained_entries > 0 && retained_entries <= MAX_ENTRIES);
            assert!(retained_bytes <= MAX_BYTES);
        }
        Ok(())
    }

    /// Cached UTF-8 decoding reports the same path and cause as the original file read.
    #[test]
    fn invalid_utf8_preserves_the_io_error() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("invalid.json5");
        fs_err::write(&path, b"\xff")?;
        let original = anyhow::Error::from(
            fs_err::read_to_string(&path)
                .err()
                .context("invalid UTF-8")?,
        );
        let cache = FileReadCache::new();
        for _ in 0..2 {
            let contents = cache.read(&path)?;
            let error = contents.as_str(&path).err().context("invalid UTF-8")?;
            assert_eq!(format!("{error:#}"), format!("{original:#}"));
        }
        Ok(())
    }
}
