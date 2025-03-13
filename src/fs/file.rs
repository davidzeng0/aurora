#![allow(clippy::unwrap_used)]
//! The implementation for [`File`]

use std::io::SeekFrom;
use std::path::Path;

use xx_core::os::fcntl::*;
use xx_core::os::stat::*;

use super::*;
use crate::io::{read, *};

/// A file handle for reading and writing files.
pub struct File {
	fd: OwnedFd,
	offset: u64
}

#[asynchronous]
impl File {
	/// Open the file specified by `path` for reading
	#[allow(clippy::impl_trait_in_params)]
	pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
		Ok(Self {
			fd: open(path.as_ref(), BitFlags::default(), 0).await?,
			offset: 0
		})
	}

	/// Open and possibly create the file specified by `path` for writing
	#[allow(clippy::impl_trait_in_params)]
	pub async fn create(path: impl AsRef<Path>) -> Result<Self> {
		Ok(Self {
			fd: open(path.as_ref(), OpenFlag::Create | OpenFlag::WriteOnly, 0).await?,
			offset: 0
		})
	}

	/// Read from the file into the buffer `buf`
	///
	/// Returns the number of bytes read.
	///
	/// # Cancel safety.
	///
	/// This function is cancel safe. Advance the buffer by the number of bytes
	/// read and resume by calling this function with the new buffer.
	pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
		read_into!(buf);

		let read = read(self.fd.as_fd(), buf, self.offset.try_into().unwrap()).await?;
		let read = check_interrupt_if_zero(read).await?;

		#[allow(clippy::arithmetic_side_effects)]
		(self.offset += read as u64);

		Ok(read)
	}

	/// Write to the file from the buffer `buf`
	///
	/// Returns the number of bytes written.
	///
	/// # Cancel safety.
	///
	/// This function is cancel safe. Advance the buffer by the number of bytes
	/// written and resume by calling this function with the new buffer.
	pub async fn write(&mut self, buf: &[u8]) -> Result<usize> {
		write_from!(buf);

		let wrote = write(self.fd.as_fd(), buf, self.offset.try_into().unwrap()).await?;
		let wrote = check_interrupt_if_zero(wrote).await?;

		#[allow(clippy::arithmetic_side_effects)]
		(self.offset += wrote as u64);

		Ok(wrote)
	}

	/// Flush written data to the disk. See [`fsync`] for more information.
	///
	/// # Cancel safety
	///
	/// This function is cancel safe. Resume the operation by calling this
	/// function.
	pub async fn flush(&mut self) -> Result<()> {
		fsync(self.fd.as_fd()).await
	}

	/// Seek the file to a specified offset.
	///
	/// See also [`SeekFrom`]
	///
	/// # Cancel safety
	///
	/// This function is cancel safe. Resume the operation by calling this
	/// function again with the same arguments if it previously failed.
	pub async fn seek(&mut self, seek: SeekFrom) -> Result<u64> {
		self.offset = match seek {
			SeekFrom::Start(pos) => pos,
			SeekFrom::Current(rel) => self.offset.checked_add_signed(rel).unwrap(),
			SeekFrom::End(rel) => self.stream_len().await?.checked_add_signed(rel).unwrap()
		};

		Ok(self.offset)
	}

	/// Close the file asynchronously. Dropping this `File` will close the file
	/// synchronously, which may not be ideal.
	pub async fn close(self) -> Result<()> {
		close(self.fd).await
	}

	/// Get the current position in the file
	#[must_use]
	pub const fn pos(&self) -> u64 {
		self.offset
	}

	/// Get the metadata for this file. See [`Metadata`] for more information
	pub async fn metadata(&self) -> Result<Metadata> {
		let mut statx = Statx::default();

		io::statx_fd(
			self.fd.as_fd(),
			BitFlags::default(),
			BitFlags::default(),
			&mut statx
		)
		.await?;

		Ok(Metadata(statx))
	}
}

#[asynchronous]
impl Read for File {
	async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
		self.read(buf).await
	}
}

#[asynchronous]
impl Write for File {
	async fn write(&mut self, buf: &[u8]) -> Result<usize> {
		self.write(buf).await
	}

	async fn flush(&mut self) -> Result<()> {
		self.flush().await
	}
}

#[asynchronous]
impl Seek for File {
	async fn seek(&mut self, seek: SeekFrom) -> Result<u64> {
		self.seek(seek).await
	}

	async fn stream_len(&mut self) -> Result<u64> {
		let stat = self.metadata().await?.0;

		if stat.mask().intersects(StatxMask::Size) {
			Ok(stat.size)
		} else {
			Err(fmt_error!("Failed to query file size"))
		}
	}

	fn stream_position_fast(&self) -> bool {
		true
	}

	async fn stream_position(&mut self) -> Result<u64> {
		Ok(self.pos())
	}
}
