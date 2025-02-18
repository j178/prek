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

use async_compression::tokio::bufread::{GzipDecoder, XzDecoder};
use async_zip::base::read::stream::ZipFileReader;
use tokio::io::{AsyncRead, BufReader};
use tokio_tar::ArchiveBuilder;
use tokio_util::compat::FuturesAsyncReadCompatExt;
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
pub async fn unzip<R: AsyncRead + Unpin>(reader: R, target: impl AsRef<Path>) -> Result<(), Error> {
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
    let mut reader = BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);
    let mut zip = ZipFileReader::with_tokio(&mut reader);

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
        use async_zip::base::read::cd::CentralDirectoryReader;
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;
        use tokio_util::compat::TokioAsyncReadCompatExt;

        let mut reader = reader.compat();
        let mut directory = CentralDirectoryReader::new(&mut reader);
        while let Some(entry) = directory.next().await? {
            if entry.dir()? {
                continue;
            }

            let Some(mode) = entry.unix_permissions() else {
                continue;
            };

            // Construct the (expected) path to the file on-disk.
            let path = entry.filename().as_str()?;
            let Some(path) = enclosed_name(path) else {
                continue;
            };
            let path = target.join(path);
            fs_err::tokio::set_permissions(&path, Permissions::from_mode(mode)).await?;
        }
    }

    Ok(())
}

/// Unpack a `.tar.gz` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unpacking files as they're being downloaded.
pub async fn untar_gz<R: AsyncRead + Unpin>(
    reader: R,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    let reader = BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);
    let reader = GzipDecoder::new(reader);

    let mut archive = ArchiveBuilder::new(reader)
        .set_preserve_mtime(true)
        .set_preserve_permissions(true)
        .build();

    archive.unpack(target.as_ref()).await?;
    Ok(())
}

/// Unpack a `.tar.xz` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unpacking files as they're being downloaded.
pub async fn untar_xz<R: AsyncRead + Unpin>(
    reader: R,
    target: impl AsRef<Path>,
) -> Result<(), Error> {
    let reader = BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);
    let reader = XzDecoder::new(reader);

    let mut archive = ArchiveBuilder::new(reader)
        .set_preserve_mtime(true)
        .set_preserve_permissions(true)
        .build();

    archive.unpack(target.as_ref()).await?;
    Ok(())
}

/// Unpack a `.tar` archive into the target directory, without requiring `Seek`.
///
/// This is useful for unpacking files as they're being downloaded.
pub async fn untar<R: AsyncRead + Unpin>(reader: R, target: impl AsRef<Path>) -> Result<(), Error> {
    let reader = BufReader::with_capacity(DEFAULT_BUF_SIZE, reader);

    let mut archive = ArchiveBuilder::new(reader)
        .set_preserve_mtime(true)
        .set_preserve_permissions(true)
        .build();

    archive.unpack(target.as_ref()).await?;
    Ok(())
}

/// Unpack a `.zip`, `.tar.gz`, `.tar.bz2`, `.tar.zst`, or `.tar.xz` archive into the target directory,
/// without requiring `Seek`.
pub async fn unpack<R: AsyncRead + Unpin>(
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
