// MIT License
//
// Copyright (c) 2023 Astral Software Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.
use std::collections::HashSet;
use std::ffi::OsString;
use std::fmt::{Display, Formatter};
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;

use futures::StreamExt;
use tokio_util::compat::{FuturesAsyncReadCompatExt, TokioAsyncReadCompatExt};
use tracing::warn;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    AsyncZip(#[from] async_zip::error::ZipError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Unsupported archive type: {0}")]
    UnsupportedArchive(PathBuf),
    #[error(
        "The top-level of the archive must only contain a list directory, but it contains: {0:?}"
    )]
    NonSingularArchive(Vec<OsString>),
    #[error("The top-level of the archive must only contain a list directory, but it's empty")]
    EmptyArchive,
}

const DEFAULT_BUF_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveExtension {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    TarZst,
    TarLzma,
    Tar,
}

impl ArchiveExtension {
    /// Extract the [`SourceDistExtension`] from a path.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        /// Returns true if the path is a tar file (e.g., `.tar.gz`).
        fn is_tar(path: &Path) -> bool {
            path.file_stem().is_some_and(|stem| {
                Path::new(stem)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("tar"))
            })
        }

        let Some(extension) = path.as_ref().extension().and_then(|ext| ext.to_str()) else {
            return Err(Error::UnsupportedArchive(path.as_ref().to_path_buf()));
        };

        match extension {
            "zip" => Ok(Self::Zip),
            "tar" => Ok(Self::Tar),
            "tgz" => Ok(Self::TarGz),
            "tbz" => Ok(Self::TarBz2),
            "txz" => Ok(Self::TarXz),
            "tlz" => Ok(Self::TarLzma),
            "gz" if is_tar(path.as_ref()) => Ok(Self::TarGz),
            "bz2" if is_tar(path.as_ref()) => Ok(Self::TarBz2),
            "xz" if is_tar(path.as_ref()) => Ok(Self::TarXz),
            "lz" | "lzma" if is_tar(path.as_ref()) => Ok(Self::TarLzma),
            "zst" if is_tar(path.as_ref()) => Ok(Self::TarZst),
            _ => Err(Error::UnsupportedArchive(path.as_ref().to_path_buf())),
        }
    }
}

impl Display for ArchiveExtension {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Zip => f.write_str("zip"),
            Self::TarGz => f.write_str("tar.gz"),
            Self::TarBz2 => f.write_str("tar.bz2"),
            Self::TarXz => f.write_str("tar.xz"),
            Self::TarZst => f.write_str("tar.zst"),
            Self::TarLzma => f.write_str("tar.lzma"),
            Self::Tar => f.write_str("tar"),
        }
    }
}

/// Extract the top-level directory from an unpacked archive.
///
/// This function returns the path to that top-level directory.
pub fn strip_component(source: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let top_level =
        fs_err::read_dir(source.as_ref())?.collect::<std::io::Result<Vec<fs_err::DirEntry>>>()?;
    match top_level.as_slice() {
        [root] => Ok(root.path()),
        [] => Err(Error::EmptyArchive),
        _ => Err(Error::NonSingularArchive(
            top_level
                .into_iter()
                .map(|entry| entry.file_name())
                .collect(),
        )),
    }
}

/// Unpack a `.zip` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unzipping files as they're being downloaded. If the archive
/// is already fully on disk, consider using `unzip_archive`, which can use multiple
/// threads to work faster in that case.
pub async fn unzip<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    /// Ensure the file path is safe to use as a [`Path`].
    ///
    /// See: <https://docs.rs/zip/latest/zip/read/struct.ZipFile.html#method.enclosed_name>
    pub(crate) fn enclosed_name(file_name: &str) -> Option<PathBuf> {
        if file_name.contains('\0') {
            return None;
        }
        let path = PathBuf::from(file_name);
        let mut depth = 0usize;
        for component in path.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => return None,
                Component::ParentDir => depth = depth.checked_sub(1)?,
                Component::Normal(_) => depth += 1,
                Component::CurDir => (),
            }
        }
        Some(path)
    }

    let target = target.as_ref();
    let mut reader = futures::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, reader.compat());
    let mut zip = async_zip::base::read::stream::ZipFileReader::new(&mut reader);

    let mut directories = HashSet::new();

    while let Some(mut entry) = zip.next_with_entry().await? {
        // Construct the (expected) path to the file on-disk.
        let path = entry.reader().entry().filename().as_str()?;

        // Sanitize the file name to prevent directory traversal attacks.
        let Some(path) = enclosed_name(path) else {
            warn!("Skipping unsafe file name: {path}");

            // Close current file prior to proceeding, as per:
            // https://docs.rs/async_zip/0.0.16/async_zip/base/read/stream/
            zip = entry.skip().await?;
            continue;
        };
        let path = target.join(path);
        let is_dir = entry.reader().entry().dir()?;

        // Either create the directory or write the file to disk.
        if is_dir {
            if directories.insert(path.clone()) {
                fs_err::tokio::create_dir_all(path).await?;
            }
        } else {
            if let Some(parent) = path.parent() {
                if directories.insert(parent.to_path_buf()) {
                    fs_err::tokio::create_dir_all(parent).await?;
                }
            }

            // We don't know the file permissions here, because we haven't seen the central directory yet.
            let file = fs_err::tokio::File::create(&path).await?;
            let size = entry.reader().entry().uncompressed_size();
            let mut writer = if let Ok(size) = usize::try_from(size) {
                tokio::io::BufWriter::with_capacity(std::cmp::min(size, 1024 * 1024), file)
            } else {
                tokio::io::BufWriter::new(file)
            };
            let mut reader = entry.reader_mut().compat();
            tokio::io::copy(&mut reader, &mut writer).await?;
        }

        // Close current file prior to proceeding, as per:
        // https://docs.rs/async_zip/0.0.16/async_zip/base/read/stream/
        zip = entry.skip().await?;
    }

    // On Unix, we need to set file permissions, which are stored in the central directory, at the
    // end of the archive. The `ZipFileReader` reads until it sees a central directory signature,
    // which indicates the first entry in the central directory. So we continue reading from there.
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;

        let mut directory = async_zip::base::read::cd::CentralDirectoryReader::new(&mut reader);
        while let Some(entry) = directory.next().await? {
            if entry.dir()? {
                continue;
            }

            let Some(mode) = entry.unix_permissions() else {
                continue;
            };

            // The executable bit is the only permission we preserve, otherwise we use the OS defaults.
            // https://github.com/pypa/pip/blob/3898741e29b7279e7bffe044ecfbe20f6a438b1e/src/pip/_internal/utils/unpacking.py#L88-L100
            let has_any_executable_bit = mode & 0o111;
            if has_any_executable_bit != 0 {
                // Construct the (expected) path to the file on-disk.
                let path = entry.filename().as_str()?;
                let Some(path) = enclosed_name(path) else {
                    continue;
                };
                let path = target.join(path);

                let permissions = fs_err::tokio::metadata(&path).await?.permissions();
                if permissions.mode() & 0o111 != 0o111 {
                    fs_err::tokio::set_permissions(
                        &path,
                        Permissions::from_mode(permissions.mode() | 0o111),
                    )
                    .await?;
                }
            }
        }
    }

    Ok(())
}

/// Determine the path at which the given tar entry will be unpacked, when unpacking into `dst`.
///
/// See: <https://github.com/vorot93/tokio-tar/blob/87338a76092330bc6fe60de95d83eae5597332e1/src/entry.rs#L418>
#[cfg_attr(not(unix), allow(dead_code))]
fn unpacked_at(dst: &Path, entry: &Path) -> Option<PathBuf> {
    let mut file_dst = dst.to_path_buf();
    {
        for part in entry.components() {
            match part {
                // Leading '/' characters, root paths, and '.'
                // components are just ignored and treated as "empty
                // components"
                Component::Prefix(..) | Component::RootDir | Component::CurDir => {
                    continue;
                }

                // If any part of the filename is '..', then skip over
                // unpacking the file to prevent directory traversal
                // security issues.  See, e.g.: CVE-2001-1267,
                // CVE-2002-0399, CVE-2005-1918, CVE-2007-4131
                Component::ParentDir => return None,

                Component::Normal(part) => file_dst.push(part),
            }
        }
    }

    // Skip cases where only slashes or '.' parts were seen, because
    // this is effectively an empty filename.
    if *dst == *file_dst {
        return None;
    }

    // Skip entries without a parent (i.e. outside of FS root)
    file_dst.parent()?;

    Some(file_dst)
}

/// Unpack the given tar archive into the destination directory.
///
/// This is equivalent to `archive.unpack_in(dst)`, but it also preserves the executable bit.
async fn untar_in<'a>(
    mut archive: tokio_tar::Archive<&'a mut (dyn tokio::io::AsyncRead + Unpin)>,
    dst: &Path,
) -> std::io::Result<()> {
    let mut entries = archive.entries()?;
    let mut pinned = Pin::new(&mut entries);
    while let Some(entry) = pinned.next().await {
        // Unpack the file into the destination directory.
        let mut file = entry?;

        // On Windows, skip symlink entries, as they're not supported. pip recursively copies the
        // symlink target instead.
        if cfg!(windows) && file.header().entry_type().is_symlink() {
            warn!(
                "Skipping symlink in tar archive: {}",
                file.path()?.display()
            );
            continue;
        }

        file.unpack_in(dst).await?;

        // Preserve the executable bit.
        #[cfg(unix)]
        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;

            let entry_type = file.header().entry_type();
            if entry_type.is_file() || entry_type.is_hard_link() {
                let mode = file.header().mode()?;
                let has_any_executable_bit = mode & 0o111;
                if has_any_executable_bit != 0 {
                    if let Some(path) = unpacked_at(dst, &file.path()?) {
                        let permissions = fs_err::tokio::metadata(&path).await?.permissions();
                        if permissions.mode() & 0o111 != 0o111 {
                            fs_err::tokio::set_permissions(
                                &path,
                                Permissions::from_mode(permissions.mode() | 0o111),
                            )
                            .await?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Unpack a `.tar.gz` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unpacking files as they're being downloaded.
pub async fn untar_gz<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    let reader = tokio::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);
    let mut decompressed_bytes = async_compression::tokio::bufread::GzipDecoder::new(reader);

    let archive = tokio_tar::ArchiveBuilder::new(
        &mut decompressed_bytes as &mut (dyn tokio::io::AsyncRead + Unpin),
    )
    .set_preserve_mtime(false)
    .build();
    Ok(untar_in(archive, target.as_ref()).await?)
}

/// Unpack a `.tar.xz` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unpacking files as they're being downloaded.
pub async fn untar_xz<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    let reader = tokio::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);
    let mut decompressed_bytes = async_compression::tokio::bufread::XzDecoder::new(reader);

    let archive = tokio_tar::ArchiveBuilder::new(
        &mut decompressed_bytes as &mut (dyn tokio::io::AsyncRead + Unpin),
    )
    .set_preserve_mtime(false)
    .build();
    untar_in(archive, target.as_ref()).await?;
    Ok(())
}

/// Unpack a `.tar` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unpacking files as they're being downloaded.
pub async fn untar<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    let mut reader = tokio::io::BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);

    let archive =
        tokio_tar::ArchiveBuilder::new(&mut reader as &mut (dyn tokio::io::AsyncRead + Unpin))
            .set_preserve_mtime(false)
            .build();
    untar_in(archive, target.as_ref()).await?;
    Ok(())
}

/// Unpack a `.zip`, `.tar.gz`, `.tar.bz2`, `.tar.zst`, or `.tar.xz` archive into the target directory,
/// without requiring `Seek`.
pub async fn unpack<R: tokio::io::AsyncRead + Unpin>(
    reader: R,
    ext: ArchiveExtension,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    match ext {
        ArchiveExtension::Zip => unzip(reader, target).await,
        ArchiveExtension::Tar => untar(reader, target).await,
        ArchiveExtension::TarGz => untar_gz(reader, target).await,
        ArchiveExtension::TarXz => untar_xz(reader, target).await,
        _ => Err(Error::UnsupportedArchive(target.as_ref().to_path_buf())),
    }
}
