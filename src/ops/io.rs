//! Direct I/O operations and syscalls.

use std::ffi::CStr;
use std::mem::size_of;
use std::os::fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::path::Path;

use xx_core::coroutines::ops::AsyncFnOnce;
use xx_core::error::*;
use xx_core::macros::paste;
use xx_core::os;
use xx_core::os::epoll::*;
use xx_core::os::fcntl::*;
use xx_core::os::inet::*;
use xx_core::os::openat::*;
use xx_core::os::socket::*;
use xx_core::os::stat::*;
use xx_core::pointer::*;

use super::*;

pub mod raw {
	//! Raw async I/O functions. Use with care. See [the documentation for the
	//! safe counterparts](`super`) for more information

	use xx_core::os::socket::raw::MsgHdr;

	use super::*;

	#[cfg(feature = "tracing")]
	#[allow(unreachable_pub)]
	mod tracing {
		use std::fmt;
		use std::marker::PhantomData;

		use enumflags2::{BitFlag, BitFlags};
		pub use xx_core::num_traits::FromPrimitive;
		pub use xx_core::pointer::*;

		/// # Safety
		/// valid cstr
		pub unsafe fn get_cstr_as_str<'a>(cstr: Ptr<()>) -> &'a str {
			/* Safety: guaranteed by caller */
			let cstr = unsafe { std::ffi::CStr::from_ptr(cstr.as_ptr().cast()) };

			cstr.to_str().unwrap_or("<error>")
		}

		pub struct EnumDisplay<T>(u32, PhantomData<T>);

		impl<T> EnumDisplay<T> {
			pub const fn new(value: u32) -> Self {
				Self(value, PhantomData)
			}
		}

		impl<T: FromPrimitive + fmt::Debug> fmt::Display for EnumDisplay<T> {
			fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
				if let Some(val) = T::from_u32(self.0) {
					fmt::Debug::fmt(&val, fmt)
				} else {
					fmt::Display::fmt(&self.0, fmt)
				}
			}
		}

		pub struct FlagsDisplay<T>(u32, PhantomData<T>);

		impl<T> FlagsDisplay<T> {
			pub const fn new(value: u32) -> Self {
				Self(value, PhantomData)
			}
		}

		impl<T: BitFlag<Numeric = u32> + Clone + fmt::Debug> fmt::Display for FlagsDisplay<T> {
			fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
				let (flags, invalid) = match BitFlags::<T>::from_bits(self.0) {
					Ok(flags) => (flags, None),
					Err(err) => (err.truncate(), Some(err.invalid_bits()))
				};

				match (flags.is_empty(), invalid) {
					(_, None) => fmt::Display::fmt(&flags, fmt)?,
					(false, Some(invalid)) => fmt::LowerHex::fmt(&invalid, fmt)?,
					(true, Some(invalid)) => {
						fmt::Display::fmt(&flags, fmt)?;
						fmt::Display::fmt(&" + ", fmt)?;
						fmt::LowerHex::fmt(&invalid, fmt)?;
					}
				}

				Ok(())
			}
		}

		impl<T: BitFlag<Numeric = u32> + Clone + fmt::Debug> fmt::Debug for FlagsDisplay<T> {
			fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
				fmt::Display::fmt(self, fmt)
			}
		}
	}

	#[cfg(feature = "tracing")]
	use self::tracing::*;

	macro_rules! async_engine_task {
		($force: literal, $func: ident ($($arg: ident: $type: ty),*) -> $return_type: ty {
			trace($($trace:tt)*) = $result:ident $($map:tt)*
		}) => {
			/// # Safety
			/// all pointers must be valid until the function returns
			#[asynchronous]
			#[inline]
			pub async unsafe fn $func($($arg: $type),*) -> $return_type {
				let driver = internal_get_driver().await;

				if !$force {
					check_interrupt().await?;

					driver.check_exiting()?;
				}

				/* Safety: guaranteed by caller */
				let result = unsafe { block_on(driver.$func($($arg),*)).await };
				let $result = paste! { Engine::[<result_for_ $func>](result) };

				#[cfg(feature = "tracing")]
				xx_core::trace!(target: driver, $($trace)*, $result $($map)*);

				$result.map_err(|err| err.into())
			}
		}
	}

	async_engine_task!(false, open(path: Ptr<()>, flags: u32, mode: u32) -> Result<OwnedFd> {
		trace(
			"## open(path = {}, flags = {}, mode = {:o}) = {:?}",
			/* Safety: guaranteed by caller */
			unsafe { get_cstr_as_str(path) },
			FlagsDisplay::<OpenFlag>::new(flags),
			mode
		) = result
	});

	async_engine_task!(true, close(fd: RawFd) -> Result<()> {
		trace("## close(fd = {}) = {:?}", fd) = result
	});

	async_engine_task!(false, read(fd: RawFd, buf: MutPtr<()>, len: usize, offset: i64) -> Result<usize> {
		trace("## read(fd = {}, buf = &mut [u8; {}], offset = {}) = {:?}", fd, len, offset) = result
	});

	async_engine_task!(false, write(fd: RawFd, buf: Ptr<()>, len: usize, offset: i64) -> Result<usize> {
		trace("## write(fd = {}, buf = &[u8; {}], offset = {}) = {:?}", fd, len, offset) = result
	});

	async_engine_task!(false, socket(domain: u32, socket_type: u32, protocol: u32) -> Result<OwnedFd> {
		trace(
			"## socket(domain = {}, socket_type = {}, protocol = {}) = {:?}",
			EnumDisplay::<AddressFamily>::new(domain),
			EnumDisplay::<SocketType>::new(socket_type),
			EnumDisplay::<IpProtocol>::new(protocol)
		) = result
	});

	async_engine_task!(false, accept(socket: RawFd, addr: MutPtr<()>, addrlen: MutPtr<i32>) -> Result<OwnedFd> {
		trace("## accept(fd = {}, addr = {:?}, addrlen = {:?}) = {:?}", socket, addr, addrlen) = result
	});

	async_engine_task!(false, connect(socket: RawFd, addr: Ptr<()>, addrlen: i32) -> Result<()> {
		trace("## connect(fd = {}, addr = {:?}, addrlen = {}) = {:?}", socket, addr, addrlen) = result
	});

	async_engine_task!(false, recv(socket: RawFd, buf: MutPtr<()>, len: usize, flags: u32) -> Result<usize> {
		trace(
			"## recv(fd = {}, buf = &mut [u8; {}], flags = {}) = {:?}",
			socket,
			len,
			FlagsDisplay::<MessageFlag>::new(flags)
		) = result
	});

	async_engine_task!(false, recvmsg(socket: RawFd, header: MutPtr<MsgHdr>, flags: u32) -> Result<usize> {
		trace(
			"## recvmsg(fd = {}, header = {:?}, flags = {}) = {:?}",
			socket,
			header,
			FlagsDisplay::<MessageFlag>::new(flags)
		) = result
	});

	async_engine_task!(false, send(socket: RawFd, buf: Ptr<()>, len: usize, flags: u32) -> Result<usize> {
		trace(
			"## send(fd = {}, buf = &[u8; {}], flags = {}) = {:?}",
			socket,
			len,
			FlagsDisplay::<MessageFlag>::new(flags)
		) = result
	});

	async_engine_task!(false, sendmsg(socket: RawFd, header: Ptr<MsgHdr>, flags: u32) -> Result<usize> {
		trace(
			"## sendmsg(fd = {}, header = {:?}, flags = {}) = {:?}",
			socket,
			header,
			FlagsDisplay::<MessageFlag>::new(flags)
		) = result
	});

	async_engine_task!(false, shutdown(socket: RawFd, how: u32) -> Result<()> {
		trace("## shutdown(fd = {}, how = {}) = {:?}", socket, EnumDisplay::<Shutdown>::new(how)) = result
	});

	async_engine_task!(false, bind(socket: RawFd, addr: Ptr<()>, addrlen: i32) -> Result<()> {
		trace("## bind(fd = {}, addr = {:?}, addrlen = {}) = {:?}", socket, addr, addrlen) = result
	});

	async_engine_task!(false, listen(socket: RawFd, backlog: i32) -> Result<()> {
		trace("## listen(fd = {}, backlog = {}) = {:?}", socket, backlog) = result
	});

	async_engine_task!(false, fsync(file: RawFd) -> Result<()> {
		trace("## fsync(fd = {}) = {:?}", file) = result
	});

	async_engine_task!(false, statx(dirfd: RawFd, path: Ptr<()>, flags: u32, mask: u32, statx: MutPtr<Statx>) -> Result<()> {
		trace(
			"## statx(dirfd = {}, path = {}, flags = {}, mask = {}, statx = {:?}) = {:?}",
			dirfd,
			/* Safety: guaranteed by caller */
			unsafe { get_cstr_as_str(path) },
			FlagsDisplay::<AtFlag>::new(flags),
			FlagsDisplay::<StatxMask>::new(mask),
			statx
		) = result
	});

	async_engine_task!(false, poll(fd: RawFd, mask: u32) -> Result<u32> {
		trace("## poll(fd = {}, mask = {}) = {:?}", fd, FlagsDisplay::<PollFlag>::new(mask)) = result
			.as_ref()
			.map(|mask| FlagsDisplay::<PollFlag>::new(*mask))
	});
}

#[asynchronous]
async fn with_path_as_cstr<F, Output>(path: impl AsRef<Path>, func: F) -> Result<Output>
where
	F: AsyncFnOnce(&CStr) -> Result<Output>
{
	let context = get_context().await;

	/* Safety: we are in an async function */
	os::with_path_as_cstr(path, |path| unsafe {
		scoped(context, func.call_once(path))
	})
}

/// The equivalent of an `open(2)` syscall. The file at `path` is opened, and a
/// file descriptor is returned.
///
/// See [`OpenFlag`] for a list of possible flags and their behaviors.
///
/// The argument `mode` is only used when creating a file, and specifies the
/// permissions.
#[asynchronous]
#[allow(clippy::impl_trait_in_params)]
pub async fn open(path: impl AsRef<Path>, flags: BitFlags<OpenFlag>, mode: u32) -> Result<OwnedFd> {
	with_path_as_cstr(path, |path: &CStr| async move {
		/* Safety: all references must be valid for this function call */
		unsafe { raw::open(ptr!(path.as_ptr()).cast(), flags.bits(), mode).await }
	})
	.await
}

/// The equivalent of a `close(2)` syscall. Closes the file descriptor `fd`.
#[asynchronous]
pub async fn close(fd: OwnedFd) -> Result<()> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::close(fd.as_raw_fd()).await }
}

/// The equivalent of a read(2) syscall. Reads from the file descriptor into the
/// buffer, with an optional offset. On files that support seeking, if the
/// offset is set to `-1`, the read operation commences at the file offset, and
/// the file offset is incremented by the number of bytes read. On files that
/// are not capable of seeking, the offset must be either `0` or `-1`.
///
/// Returns the number of bytes read.
#[asynchronous]
pub async fn read(fd: BorrowedFd<'_>, buf: &mut [u8], offset: i64) -> Result<usize> {
	/* Safety: all references must be valid for this function call */
	unsafe {
		raw::read(
			fd.as_raw_fd(),
			ptr!(buf.as_mut_ptr()).cast(),
			buf.len(),
			offset
		)
		.await
	}
}

/// The equivalent of a `write(2)` syscall. Write to the file descriptor from
/// the buffer, with an optional offset. On files that support seeking, if the
/// offset is set to `-1`, the write operation commences at the file offset, and
/// the file offset is incremented by the number of bytes read. On files that
/// are not capable of seeking, the offset must be either `0` or `-1`.
///
/// Returns the number of bytes written.
#[asynchronous]
pub async fn write(fd: BorrowedFd<'_>, buf: &[u8], offset: i64) -> Result<usize> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::write(fd.as_raw_fd(), ptr!(buf.as_ptr()).cast(), buf.len(), offset).await }
}

/// The equivalent of a `socket(2)` syscall. A socket is created matching the
/// `domain`, `socket_type` and `protocol` arguments
#[asynchronous]
pub async fn socket(
	domain: AddressFamily, socket_type: u32, protocol: IpProtocol
) -> Result<OwnedFd> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::socket(domain as u32, socket_type, protocol as u32).await }
}

/// The equivalent of an `accept(2)` syscall. Accepts an incoming connection on
/// a socket. The `addr` argument is used to store the address of the incoming
/// connection. Returns a tuple of the socket file descriptor and the length in
/// bytes of the address
///
/// # Safety
/// `addr` must be valid for stores of socket addresses
#[asynchronous]
pub async unsafe fn accept<A>(socket: BorrowedFd<'_>, addr: &mut A) -> Result<(OwnedFd, i32)> {
	#[allow(clippy::unwrap_used)]
	let mut addrlen = size_of::<A>().try_into().unwrap();

	/* Safety: all references must be valid for this function call */
	let fd =
		unsafe { raw::accept(socket.as_raw_fd(), ptr!(addr).cast(), ptr!(&mut addrlen)).await? };

	Ok((fd, addrlen))
}

/// The equivalent of a `connect(2)` syscall. Connects the socket to the address
/// specified by `addr`
#[asynchronous]
pub async fn connect<A>(socket: BorrowedFd<'_>, addr: &A) -> Result<()> {
	/* Safety: all references must be valid for this function call */
	#[allow(clippy::unwrap_used)]
	unsafe {
		raw::connect(
			socket.as_raw_fd(),
			ptr!(addr).cast(),
			size_of::<A>().try_into().unwrap()
		)
		.await
	}
}

/// The same as [`connect`]
#[asynchronous]
pub async fn connect_addr(socket: BorrowedFd<'_>, addr: &Address) -> Result<()> {
	match &addr {
		Address::V4(addr) => connect(socket, addr).await,
		Address::V6(addr) => connect(socket, addr).await
	}
}

/// The equivalent of a `recv(2)` syscall. Receives data from the socket into
/// `buf`.
///
/// See [`MessageFlag`] for a list of possible flags and their behaviors.
///
/// Returns the number of bytes read.
#[asynchronous]
pub async fn recv(
	socket: BorrowedFd<'_>, buf: &mut [u8], flags: BitFlags<MessageFlag>
) -> Result<usize> {
	/* Safety: all references must be valid for this function call */
	unsafe {
		raw::recv(
			socket.as_raw_fd(),
			ptr!(buf.as_mut_ptr()).cast(),
			buf.len(),
			flags.bits()
		)
		.await
	}
}

/// The equivalent of a `recvmsg(2)` syscall. Receives data from the socket into
/// a buffer specified by the [`MsgHdrMut`]
///
/// See [`MessageFlag`] for a list of possible flags and their behaviors.
///
/// Returns the number of bytes read.
#[asynchronous]
pub async fn recvmsg(
	socket: BorrowedFd<'_>, header: &mut MsgHdrMut<'_>, flags: BitFlags<MessageFlag>
) -> Result<usize> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::recvmsg(socket.as_raw_fd(), ptr!(header).cast(), flags.bits()).await }
}

/// The equivalent of a `send(2)` syscall. Receives data from the socket into
/// `buf`
///
/// See [`MessageFlag`] for a list of possible flags and their behaviors.
///
/// Returns the number of bytes sent.
#[asynchronous]
pub async fn send(
	socket: BorrowedFd<'_>, buf: &[u8], flags: BitFlags<MessageFlag>
) -> Result<usize> {
	/* Safety: all references must be valid for this function call */
	unsafe {
		raw::send(
			socket.as_raw_fd(),
			ptr!(buf.as_ptr()).cast(),
			buf.len(),
			flags.bits()
		)
		.await
	}
}

/// The equivalent of a `sendmsg(2)` syscall. Receives data from the socket into
/// a buffer specified by the [`MsgHdr`]
///
/// See [`MessageFlag`] for a list of possible flags and their behaviors.
///
/// Returns the number of bytes sent.
#[asynchronous]
pub async fn sendmsg(
	socket: BorrowedFd<'_>, header: &MsgHdr<'_>, flags: BitFlags<MessageFlag>
) -> Result<usize> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::sendmsg(socket.as_raw_fd(), ptr!(header).cast(), flags.bits()).await }
}

/// The equivalent of a `shutdown(2)` syscall. Shuts down a part or all of the
/// connection according to the `how` argument.
///
/// See [`Shutdown`] for possible values
#[asynchronous]
pub async fn shutdown(socket: BorrowedFd<'_>, how: Shutdown) -> Result<()> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::shutdown(socket.as_raw_fd(), how as u32).await }
}

/// The equivalent of a `bind(2)` syscall. Assign an address to the socket.
#[asynchronous]
pub async fn bind<A>(socket: BorrowedFd<'_>, addr: &A) -> Result<()> {
	/* Safety: all references must be valid for this function call */
	#[allow(clippy::unwrap_used)]
	unsafe {
		raw::bind(
			socket.as_raw_fd(),
			ptr!(addr).cast(),
			size_of::<A>().try_into().unwrap()
		)
		.await
	}
}

/// The same as [`bind`]
#[asynchronous]
pub async fn bind_addr(socket: BorrowedFd<'_>, addr: &Address) -> Result<()> {
	match &addr {
		Address::V4(addr) => bind(socket, addr).await,
		Address::V6(addr) => bind(socket, addr).await
	}
}

/// The equivalent of a `listen(2)` syscall. The socket is marked as a passive
/// socket and can be used to accept incoming connection requests using
/// [`accept`]
///
/// The `backlog` argument is the maximum length to which the queue of pending
/// connections for the socket may grow to.
#[asynchronous]
pub async fn listen(socket: BorrowedFd<'_>, backlog: i32) -> Result<()> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::listen(socket.as_raw_fd(), backlog).await }
}

/// The equivalent of an `fsync(2)` syscall. Modifications to the file are
/// flushed to the disk.
#[asynchronous]
pub async fn fsync(file: BorrowedFd<'_>) -> Result<()> {
	/* Safety: all references must be valid for this function call */
	unsafe { raw::fsync(file.as_raw_fd()).await }
}

/// The equivalent of an `statx(2)` syscall. Information about the file is
/// returned in the `statx` argument. See [`Statx`] for more info.
///
/// The optional `dirfd` argument specifies the directory to which `path` is
/// relative to. If not specified, the path is relative to the process's current
/// working directory.
///
/// See [`AtFlag`] for a list of possible flags and their behaviors.
///
/// The `mask` argument tells the kernel what fields the caller is interested
/// in. See [`StatxMask`] for a list of possible flags.
#[asynchronous]
#[allow(clippy::impl_trait_in_params)]
pub async fn statx(
	dirfd: Option<BorrowedFd<'_>>, path: impl AsRef<Path>, flags: BitFlags<AtFlag>,
	mask: BitFlags<StatxMask>, statx: &mut Statx
) -> Result<()> {
	let dirfd = into_raw_dirfd(dirfd);

	with_path_as_cstr(path, |path: &CStr| async move {
		/* Safety: all references must be valid for this function call */
		unsafe {
			raw::statx(
				dirfd,
				ptr!(path.as_ptr()).cast(),
				flags.bits(),
				mask.bits(),
				statx.into()
			)
			.await
		}
	})
	.await
}

/// The same as [`statx`] but instead of a file path, a file descriptor is used
/// instead as the target.
#[asynchronous]
#[allow(clippy::impl_trait_in_params)]
pub async fn statx_fd(
	fd: BorrowedFd<'_>, mut flags: BitFlags<AtFlag>, mask: BitFlags<StatxMask>, statx: &mut Statx
) -> Result<()> {
	const EMPTY_PATH: &CStr = c"";

	flags |= AtFlag::EmptyPath;

	/* Safety: all references must be valid for this function call */
	unsafe {
		raw::statx(
			fd.as_raw_fd(),
			ptr!(EMPTY_PATH.as_ptr()).cast(),
			flags.bits(),
			mask.bits(),
			statx.into()
		)
		.await
	}
}

/// Wait for an event on a file descriptor.
///
/// See [`PollFlag`] for a list of possible events.
///
/// Returns the events that were notified.
#[asynchronous]
pub async fn poll(fd: BorrowedFd<'_>, mask: BitFlags<PollFlag>) -> Result<BitFlags<PollFlag>> {
	/* Safety: all references must be valid for this function call */
	let bits = unsafe { raw::poll(fd.as_raw_fd(), mask.bits()).await? };

	Ok(BitFlags::from_bits_truncate(bits))
}
