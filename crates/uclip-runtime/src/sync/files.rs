use std::{
    collections::HashSet,
    fs::{self, File, Metadata, OpenOptions},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use uclip_core::{AppError, ErrorCode};

use super::{
    transport::{invalid, limit},
    wire::Wire,
};

const MAX_FILES: usize = 8192;
const MAX_DEPTH: usize = 32;
pub(super) const MAX_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct FileEntry {
    root: usize,
    parts: Vec<String>,
    /// None describes a directory. A zero-length file still has a length.
    bytes: Option<u64>,
}

pub(super) struct Source {
    entries: Vec<FileEntry>,
    paths: Vec<PathBuf>,
}

fn missing() -> AppError {
    AppError::new(ErrorCode::MissingFile)
}

fn storage() -> AppError {
    AppError::new(ErrorCode::Storage)
}

fn file_name(value: &str) -> bool {
    let first = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !value.is_empty()
        && value.len() <= 240
        && !value.ends_with(['.', ' '])
        && !value
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
        && !matches!(
            first.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        && !["COM", "LPT"].iter().any(|prefix| {
            first.strip_prefix(prefix).is_some_and(|tail| {
                matches!(
                    tail,
                    "1" | "2"
                        | "3"
                        | "4"
                        | "5"
                        | "6"
                        | "7"
                        | "8"
                        | "9"
                        | "\u{b9}"
                        | "\u{b2}"
                        | "\u{b3}"
                )
            })
        })
}

pub(super) fn safe_metadata(path: &Path) -> Result<Metadata, AppError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| missing())?;
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(missing());
        }
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(missing());
    }
    Ok(metadata)
}

impl Source {
    pub fn open(paths: Vec<String>) -> Result<Self, AppError> {
        let mut source = Self {
            entries: Vec::new(),
            paths: Vec::new(),
        };
        let mut bytes = 0;
        for (index, path) in paths.iter().enumerate() {
            let path = Path::new(path);
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(missing)?;
            source.walk(index, path, vec![name.into()], &mut bytes)?;
        }
        validate(&source.entries, MAX_BYTES)?;
        Ok(source)
    }

    fn walk(
        &mut self,
        root: usize,
        path: &Path,
        parts: Vec<String>,
        total: &mut u64,
    ) -> Result<(), AppError> {
        if self.entries.len() >= MAX_FILES || parts.len() > MAX_DEPTH {
            return Err(limit());
        }
        if !parts.last().is_some_and(|part| file_name(part)) {
            return Err(missing());
        }
        let meta = safe_metadata(path)?;
        let bytes = meta.is_file().then_some(meta.len());
        *total = total
            .checked_add(bytes.unwrap_or_default())
            .filter(|total| *total <= MAX_BYTES)
            .ok_or_else(limit)?;
        self.entries.push(FileEntry {
            root,
            parts: parts.clone(),
            bytes,
        });
        self.paths.push(path.into());
        if meta.is_dir() {
            for entry in fs::read_dir(path).map_err(|_| missing())? {
                let entry = entry.map_err(|_| missing())?;
                let mut child = parts.clone();
                child.push(entry.file_name().into_string().map_err(|_| missing())?);
                self.walk(root, &entry.path(), child, total)?;
            }
        }
        Ok(())
    }

    pub fn entries(&self) -> &[FileEntry] {
        &self.entries
    }

    pub fn send(&self, wire: &mut Wire, stop: &AtomicBool) -> Result<(), AppError> {
        for (entry, path) in self.entries.iter().zip(&self.paths) {
            if let Some(size) = entry.bytes {
                if safe_metadata(path)?.len() != size {
                    return Err(missing());
                }
                let mut file = File::open(path).map_err(|_| missing())?;
                if !file
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.len() == size)
                {
                    return Err(missing());
                }
                wire.send_file(&mut file, size, stop)?;
                if !file
                    .metadata()
                    .is_ok_and(|meta| meta.is_file() && meta.len() == size)
                {
                    return Err(missing());
                }
            }
        }
        Ok(())
    }
}

fn validate(entries: &[FileEntry], budget: u64) -> Result<u64, AppError> {
    if entries.is_empty() || entries.len() > MAX_FILES {
        return Err(limit());
    }
    let mut roots = 0;
    let mut directories = HashSet::new();
    let mut used = HashSet::new();
    let mut total = 0u64;
    for entry in entries {
        if entry.root >= 256
            || entry.parts.is_empty()
            || entry.parts.len() > MAX_DEPTH
            || !entry.parts.iter().all(|name| file_name(name))
        {
            return Err(invalid());
        }
        let key = entry
            .parts
            .iter()
            .map(|part| part.to_lowercase())
            .collect::<Vec<_>>();
        if key.len() == 1 {
            if entry.root != roots {
                return Err(invalid());
            }
            roots += 1;
        } else if !directories.contains(&(entry.root, key[..key.len() - 1].to_vec())) {
            return Err(invalid());
        }
        if !used.insert((entry.root, key.clone())) {
            return Err(invalid());
        }
        if let Some(size) = entry.bytes {
            total = total
                .checked_add(size)
                .filter(|total| *total <= budget)
                .ok_or_else(limit)?;
        } else {
            directories.insert((entry.root, key));
        }
    }
    Ok(total)
}

pub(super) struct Received {
    pub directory: TempDir,
    pub paths: Vec<String>,
    pub bytes: u64,
}

pub(super) fn receive(
    root: &Path,
    entries: Vec<FileEntry>,
    wire: &mut Wire,
    budget: u64,
) -> Result<Received, AppError> {
    let bytes = validate(&entries, budget)?;
    let directory = tempfile::Builder::new()
        .prefix("received-")
        .tempdir_in(root)
        .map_err(|_| storage())?;
    let mut paths = Vec::new();
    for entry in entries {
        let slot = directory.path().join(entry.root.to_string());
        if entry.parts.len() == 1 {
            fs::create_dir(&slot).map_err(|_| storage())?;
        }
        let path = entry
            .parts
            .iter()
            .fold(slot, |path, segment| path.join(segment));
        if let Some(bytes) = entry.bytes {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| storage())?;
            wire.receive_file(&mut file, bytes)?;
            file.sync_all().map_err(|_| storage())?;
        } else {
            fs::create_dir(&path).map_err(|_| storage())?;
        }
        if entry.parts.len() == 1 {
            paths.push(path.to_str().ok_or_else(storage)?.to_string());
        }
    }
    Ok(Received {
        directory,
        paths,
        bytes,
    })
}
