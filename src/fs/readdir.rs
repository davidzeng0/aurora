//! The implementation for [`read_dir`]

use std::ffi::OsStr;
use std::fmt;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use enumflags2::make_bitflags;
use xx_core::async_std::AsyncIterator;
use xx_core::error::*;
use xx_core::os::dirent::{DirEnts, DirentDef};
use xx_core::os::error::OsError;
use xx_core::os::fcntl::*;
use xx_core::os::stat::*;

use super::*;

struct Dir {
	path: PathBuf,
	fd: OwnedFd
}

/// An entry of the directory
pub struct DirEntry {
	dir: Arc<Dir>,
	ent: DirentDef<PathBuf>
}

#[asynchronous]
impl DirEntry {
	/// The path of the file
	#[must_use]
	pub fn path(&self) -> PathBuf {
		self.dir.path.join(self.file_name())
	}

	/// The name of the file without any leading path component(s).
	#[must_use]
	pub fn file_name(&self) -> &OsStr {
		self.ent.name.as_os_str()
	}

	#[must_use]
	pub const fn ino(&self) -> u64 {
		self.ent.ino
	}

	/// Get the metadata for this file. See [`Metadata`] for more information
	pub async fn metadata(&self) -> Result<Metadata> {
		let mut statx = Statx::default();

		io::statx(
			Some(self.dir.fd.as_fd()),
			self.file_name(),
			BitFlags::default(),
			BitFlags::default(),
			&mut statx
		)
		.await?;

		Ok(Metadata(statx))
	}

	/// Get the file type
	#[allow(clippy::unwrap_used, clippy::missing_panics_doc)]
	#[must_use]
	pub fn file_type(&self) -> FileType {
		FileType(self.ent.file_type().unwrap())
	}
}

impl fmt::Debug for DirEntry {
	fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
		let mut this = fmt.debug_struct("DirEntry");

		this.field("path", &self.path());
		this.field("inode", &self.ent.ino);
		this.field("offset", &self.ent.off);

		if let Some(ty) = self.ent.file_type() {
			this.field("type", &ty);
		} else {
			this.field("type", &self.ent.ty);
		}

		this.finish()
	}
}

/// An iterator over the files of a directory. See [`read_dir`] for more
/// information.
pub struct ReadDir {
	dir: Arc<Dir>,
	entries: DirEnts
}

#[asynchronous]
impl ReadDir {
	async fn next(&mut self) -> Result<Option<DirEntry>> {
		while !self.entries.is_eof() {
			if !self.entries.has_next_cached() {
				run_blocking(|state| loop {
					let result = self.entries.read_from_fd(self.dir.fd.as_fd());

					if !matches!(result, Err(OsError::Intr)) || state.cancelled() {
						break result;
					}
				})
				.await??;

				continue;
			}

			#[allow(clippy::unwrap_used)]
			let entry = self.entries.next_entry().unwrap();

			if entry.name == c"." || entry.name == c".." {
				continue;
			}

			let name = OsStr::from_bytes(entry.name.to_bytes())
				.to_os_string()
				.into();

			let ent = DirEntry {
				dir: self.dir.clone(),
				ent: DirentDef {
					ino: entry.ino,
					off: entry.off,
					reclen: entry.reclen,
					ty: entry.ty,
					name
				}
			};

			return Ok(Some(ent));
		}

		Ok(None)
	}
}

impl fmt::Debug for ReadDir {
	fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt.debug_struct("ReadDir")
			.field("fd", &self.dir.fd.as_raw_fd())
			.field("path", &self.dir.path)
			.finish()
	}
}

#[asynchronous]
impl AsyncIterator for ReadDir {
	type Item = Result<DirEntry>;

	/// Get the next entry in this directory. Returns `None` if there are no
	/// more.
	///
	/// # Cancel safety
	///
	/// This function is cancel safe.
	async fn next(&mut self) -> Option<Self::Item> {
		self.next().await.transpose()
	}
}

/// The async equivalent of [`std::fs::read_dir`]. Returns an iterator over the
/// files in this directory.
#[asynchronous]
#[allow(clippy::impl_trait_in_params)]
pub async fn read_dir(path: impl AsRef<Path>) -> Result<ReadDir> {
	let path = path.as_ref();
	let flags = make_bitflags!(OpenFlag::{
		Directory | LargeFile | CloseOnExec | NonBlock
	});

	let fd = io::open(path, flags, 0).await?;
	let mut statx = Statx::default();

	io::statx_fd(
		fd.as_fd(),
		BitFlags::default(),
		BitFlags::default(),
		&mut statx
	)
	.await?;

	if statx.file_type() != Some(dirent::FileType::Directory) {
		return Err(OsError::NotDir.into());
	}

	let entries = DirEnts::new_from_block_size(statx.block_size as usize);

	Ok(ReadDir {
		dir: Arc::new(Dir { path: path.to_owned(), fd }),
		entries
	})
}
