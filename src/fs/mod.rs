//! File-system operations.

use std::os::fd::{AsFd, OwnedFd};
use std::path::Path;

use xx_core::async_std::io::{ReadExt, SeekExt, *};
use xx_core::error::*;
use xx_core::os::dirent;
use xx_core::os::stat::*;

use super::*;

pub mod file;
pub mod readdir;

#[doc(inline)]
pub use {file::*, readdir::*};

/// The type of a file, obtained from a file's [`Metadata`]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FileType(dirent::FileType);

impl FileType {
	/// Returns `true` if this file is a directory
	#[must_use]
	pub fn is_dir(&self) -> bool {
		self.0 == dirent::FileType::Directory
	}

	/// Returns `true` if this file is a regular file
	#[must_use]
	pub fn is_file(&self) -> bool {
		self.0 == dirent::FileType::Regular
	}

	/// Returns `true` if this file is a symlink
	#[must_use]
	pub fn is_symlink(&self) -> bool {
		self.0 == dirent::FileType::Link
	}
}

/// The metadata of a file
#[allow(missing_copy_implementations)]
#[derive(Clone)]
pub struct Metadata(Statx);

#[allow(clippy::len_without_is_empty, clippy::missing_panics_doc)]
impl Metadata {
	/// Get the type of this file
	#[allow(clippy::unwrap_used)]
	#[must_use]
	pub fn file_type(&self) -> FileType {
		FileType(self.0.file_type().unwrap())
	}

	/// Get the file length
	#[must_use]
	pub fn len(&self) -> u64 {
		assert!(self.0.mask().intersects(StatxMask::Size));

		self.0.size
	}
}

/// Read all data from the file at `path`, appending it to the buffer `vec`
#[asynchronous]
#[allow(clippy::impl_trait_in_params, clippy::unwrap_used)]
pub async fn read_to_end(path: impl AsRef<Path>, vec: &mut Vec<u8>) -> Result<usize> {
	let mut file = File::open(path).await?;

	if let Ok(len) = file.stream_len().await {
		vec.reserve(len.try_into().unwrap());
	}

	file.read_to_end(vec).await
}

/// Load all the data in the file at `path` into a `Vec<u8>`
#[asynchronous]
#[allow(clippy::impl_trait_in_params)]
pub async fn read(path: impl AsRef<Path>) -> Result<Vec<u8>> {
	let mut vec = Vec::new();

	read_to_end(path, &mut vec).await?;

	Ok(vec)
}

/// Read the entire contents of the file into a string
#[asynchronous]
#[allow(clippy::impl_trait_in_params)]
pub async fn read_to_string(path: impl AsRef<Path>) -> Result<String> {
	Ok(String::from_utf8(read(path).await?)?)
}
