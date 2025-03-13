//! Common sockets and streams

use std::net::{SocketAddr, ToSocketAddrs};
use std::os::fd::AsRawFd;

use xx_core::coroutines::ops::{AsyncFn, AsyncFnExt, AsyncFnOnce};
use xx_core::macros::*;
use xx_core::os::epoll::PollFlag;
use xx_core::os::error::OsError;
use xx_core::os::inet::*;
use xx_core::os::socket::*;
use xx_core::pointer::*;
use xx_core::trace;

use super::*;

#[asynchronous]
async fn foreach_addr<A, F, Output>(addrs: A, f: F) -> Result<Output>
where
	A: ToSocketAddrs,
	F: AsyncFn(Address) -> Result<Output>
{
	let mut error = None;

	for addr in addrs.to_socket_addrs()? {
		match f.call(addr.into()).await {
			Ok(out) => return Ok(out),
			Err(err) => error = Some(err)
		}
	}

	Err(error.unwrap_or_else(|| common::NO_ADDRESSES.into()))
}

#[asynchronous]
async fn bind_addr<A>(addr: A, socket_type: u32, protocol: IpProtocol) -> Result<Socket>
where
	A: ToSocketAddrs
{
	foreach_addr(addr, |addr| async move {
		let sock = Socket::new_for_addr(&addr, socket_type, protocol).await?;

		set_reuse_addr(sock.fd(), true)?;
		io::bind_addr(sock.fd(), &addr).await?;

		Ok(sock)
	})
	.await
}

#[asynchronous]
async fn connect_addrs<A>(addr: A, socket_type: u32, protocol: IpProtocol) -> Result<Socket>
where
	A: ToSocketAddrs
{
	foreach_addr(addr, |addr| async move {
		let sock = Socket::new_for_addr(&addr, socket_type, protocol).await?;

		sock.connect(&addr).await?;

		Ok(sock)
	})
	.await
}

#[allow(clippy::unwrap_used, clippy::missing_panics_doc)]
fn convert_addr(storage: AddressStorage) -> SocketAddr {
	/* into should be ok here unless OS is broken */
	storage.try_into().unwrap()
}

#[asynchronous]
async fn with_budget<T, U, Sync, Suspend>(
	fd: BorrowedFd<'_>, ready: &mut BitFlags<PollFlag>, mut data: T, flags: BitFlags<PollFlag>,
	sync: Sync, suspend: Suspend
) -> Result<U>
where
	Sync: FnOnce(BorrowedFd<'_>, &mut T) -> OsResult<U>,
	Suspend: AsyncFnOnce(BorrowedFd<'_>, &mut T) -> Result<U>
{
	if ready.contains(flags) && acquire_budget(None).await {
		check_interrupt().await?;

		match sync(fd, &mut data) {
			Ok(result) => return Ok(result),
			Err(OsError::WouldBlock) => ready.remove(flags),
			Err(err) => return Err(err.into())
		}
	}

	let result = suspend.call_once((fd, &mut data)).await;

	if result.is_ok() {
		ready.insert(flags);
	}

	result
}

macro_rules! sync_io {
	($this:expr, $func:ident, $fd:expr, $buf:expr, $flags:ident, $($trace:tt)*) => {{
		let $flags = $flags | MessageFlag::DontWait | MessageFlag::NoSignal;
		let result = $func($fd, $buf, $flags);

		if result != Err(OsError::WouldBlock) {
			trace!(target: $this, $($trace)*, result);
		}

		result
	}}
}

macro_rules! sync_buf_io {
	($this:expr, $func:ident, $fd:expr, $buf:expr, $flags:ident) => {
		sync_io!(
			$this,
			$func,
			$fd,
			(*$buf).into(),
			$flags,
			"## sync_{}(fd = {}, buf = &{}[u8; {}], flags = {}) = {:?}",
			stringify!($func),
			$fd.as_raw_fd(),
			if stringify!($func) == "send" {
				""
			} else {
				"mut "
			},
			$buf.len(),
			$flags
		)
	};
}

macro_rules! sync_hdr_io {
	($this:expr, $func:ident, $fd:expr, $hdr:expr, $flags:ident) => {
		sync_io!(
			$this,
			$func,
			$fd,
			$hdr,
			$flags,
			"## sync_{}(fd = {}, header = {:?}, flags = {}) = {:?}",
			stringify!($func),
			$fd.as_raw_fd(),
			ptr!((&*$hdr)),
			$flags
		)
	};
}

macro_rules! impl_common {
	($type:ident $($generics:tt)*) => {
		#[asynchronous]
		impl $($generics)* $type $($generics)* {
			pub async fn recv(
				&mut self, buf: &mut [u8], flags: BitFlags<MessageFlag>
			) -> Result<usize> {
				read_into!(buf);

				let this = ptr!(&*self);

				with_budget(
					self.fd.as_fd(),
					&mut self.ready,
					buf,
					PollFlag::In.into(),
					/* Safety: buf is valid */
					|fd: BorrowedFd<'_>, buf: &mut &mut [u8]| unsafe {
						sync_buf_io!(this, recv, fd, buf, flags)
					},
					|fd: BorrowedFd<'_>, buf: &mut &mut [u8]| async move {
						check_interrupt_if_zero(io::recv(fd, buf, flags).await?).await
					}
				)
				.await
			}

			pub async fn recv_vectored(
				&mut self, bufs: &mut [IoSliceMut<'_>], flags: BitFlags<MessageFlag>
			) -> Result<usize> {
				let mut header = MsgHdrMut::default();

				header.set_vecs(IoVecMut::from_io_slices_mut(bufs));

				self.recvmsg(&mut header, flags).await
			}

			pub async fn recvmsg(
				&mut self, header: &mut MsgHdrMut<'_>, flags: BitFlags<MessageFlag>
			) -> Result<usize> {
				let this = ptr!(&*self);

				with_budget(
					self.fd.as_fd(),
					&mut self.ready,
					header,
					PollFlag::In.into(),
					|fd: BorrowedFd<'_>, header: &mut &mut MsgHdrMut<'_>| {
						sync_hdr_io!(this, recvmsg, fd, header, flags)
					},
					|fd: BorrowedFd<'_>, buf: &mut &mut MsgHdrMut<'_>| async move {
						check_interrupt_if_zero(io::recvmsg(fd, buf, flags).await?).await
					}
				)
				.await
			}

			pub async fn send(
				&mut self, buf: &[u8], flags: BitFlags<MessageFlag>
			) -> Result<usize> {
				write_from!(buf);

				let this = ptr!(&*self);

				with_budget(
					self.fd.as_fd(),
					&mut self.ready,
					buf,
					PollFlag::Out.into(),
					/* Safety: buf is valid */
					|fd: BorrowedFd<'_>, buf: &mut &[u8]| unsafe {
						sync_buf_io!(this, send, fd, buf, flags)
					},
					|fd: BorrowedFd<'_>, buf: &mut &[u8]| async move {
						check_interrupt_if_zero(io::send(fd, buf, flags).await?).await
					}
				)
				.await
			}

			pub async fn sendmsg(
				&mut self, header: &MsgHdr<'_>, flags: BitFlags<MessageFlag>
			) -> Result<usize> {
				let this = ptr!(&*self);

				with_budget(
					self.fd.as_fd(),
					&mut self.ready,
					header,
					PollFlag::Out.into(),
					|fd: BorrowedFd<'_>, header: &mut &MsgHdr<'_>| {
						sync_hdr_io!(this, sendmsg, fd, header, flags)
					},
					|fd: BorrowedFd<'_>, buf: &mut &MsgHdr<'_>| async move {
						check_interrupt_if_zero(io::sendmsg(fd, buf, flags).await?).await
					}
				)
				.await
			}

			pub async fn send_vectored(
				&mut self, bufs: &[IoSlice<'_>], flags: BitFlags<MessageFlag>
			) -> Result<usize> {
				let mut header = MsgHdr::default();

				header.set_vecs(IoVec::from_io_slices(bufs));

				self.sendmsg(&header, flags).await
			}

			pub async fn recvfrom(
				&mut self, buf: &mut [u8], flags: BitFlags<MessageFlag>
			) -> Result<(usize, SocketAddr)> {
				let mut addr = AddressStorage::default();
				let mut vecs = [IoVecMut::from(buf)];
				let mut header = MsgHdrMut::default();

				header.set_addr(&mut addr);
				header.set_vecs(&mut vecs[..]);

				let recvd = self.recvmsg(&mut header, flags).await?;

				Ok((recvd, convert_addr(addr)))
			}

			pub async fn sendto(
				&mut self, buf: &[u8], flags: BitFlags<MessageFlag>, addr: &SocketAddr
			) -> Result<usize> {
				write_from!(buf);

				let mut header = MsgHdr::default();
				let vecs = [IoVec::from(buf)];
				let addr = (*addr).into();

				match &addr {
					Address::V4(addr) => header.set_addr(addr),
					Address::V6(addr) => header.set_addr(addr)
				}

				header.set_vecs(&vecs[..]);

				self.sendmsg(&header, flags).await
			}

			pub async fn poll(&mut self, flags: BitFlags<PollFlag>) -> Result<BitFlags<PollFlag>> {
				self.ready.remove(flags);

				let result = io::poll(self.fd.as_fd(), flags).await?;

				self.ready.insert(result);

				Ok(result)
			}

			pub async fn shutdown(&mut self, how: Shutdown) -> Result<()> {
				io::shutdown(self.fd(), how).await?;

				let flags = match how {
					Shutdown::Read => PollFlag::In.into(),
					Shutdown::Write => PollFlag::Out.into(),
					Shutdown::Both => PollFlag::In | PollFlag::Out
				};

				self.ready.insert(flags);

				Ok(())
			}
		}

		#[asynchronous]
		#[allow(single_use_lifetimes)]
		impl $($generics)* Read for $type $($generics)* {
			async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
				self.recv(buf, BitFlags::default()).await
			}

			fn is_read_vectored(&self) -> bool {
				true
			}

			async fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> Result<usize> {
				self.recv_vectored(bufs, BitFlags::default()).await
			}
		}

		#[asynchronous]
		#[allow(single_use_lifetimes)]
		impl $($generics)* Write for $type $($generics)* {
			async fn write(&mut self, buf: &[u8]) -> Result<usize> {
				self.send(buf, BitFlags::default()).await
			}

			async fn flush(&mut self) -> Result<()> {
				/* sockets don't need flushing. set nodelay if you want immediate writes */
				Ok(())
			}

			fn is_write_vectored(&self) -> bool {
				true
			}

			async fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> Result<usize> {
				self.send_vectored(bufs, BitFlags::default()).await
			}
		}
	};
}

macro_rules! socket_common {
	() => {
		wrapper_functions! {
			inner = self.socket;

			#[must_use]
			pub fn fd(&self) -> BorrowedFd<'_>;

			#[asynchronous]
			pub async fn close(self) -> Result<()>;

			#[asynchronous]
			pub async fn recv(&mut self, buf: &mut [u8], flags: BitFlags<MessageFlag>) -> Result<usize>;

			#[asynchronous]
			pub async fn recv_vectored(&mut self, bufs: &mut [IoSliceMut<'_>], flags: BitFlags<MessageFlag>) -> Result<usize>;

			#[asynchronous]
			pub async fn recvfrom(&mut self, buf: &mut [u8], flags: BitFlags<MessageFlag>) -> Result<(usize, SocketAddr)>;

			#[asynchronous]
			pub async fn recvmsg(&mut self, header: &mut MsgHdrMut<'_>, flags: BitFlags<MessageFlag>) -> Result<usize>;

			#[asynchronous]
			pub async fn send(&mut self, buf: &[u8], flags: BitFlags<MessageFlag>) -> Result<usize>;

			#[asynchronous]
			pub async fn send_vectored(&mut self, bufs: &[IoSlice<'_>], flags: BitFlags<MessageFlag>) -> Result<usize>;

			#[asynchronous]
			pub async fn sendmsg(&mut self, header: &MsgHdr<'_>, flags: BitFlags<MessageFlag>) -> Result<usize>;

			#[asynchronous]
			pub async fn sendto(&mut self, buf: &[u8], flags: BitFlags<MessageFlag>, addr: &SocketAddr) -> Result<usize>;

			#[asynchronous]
			pub async fn poll(&mut self, flags: BitFlags<PollFlag>) -> Result<BitFlags<PollFlag>>;

			#[asynchronous]
			pub async fn shutdown(&mut self, how: Shutdown) -> Result<()>;

			#[asynchronous]
			pub async fn set_recvbuf_size(&self, size: i32) -> Result<()>;

			#[asynchronous]
			pub async fn set_sendbuf_size(&self, size: i32) -> Result<()>;

			#[asynchronous]
			pub async fn local_addr(&self) -> Result<SocketAddr>;

			#[asynchronous]
			pub async fn peer_addr(&self) -> Result<SocketAddr>;
		}

		pub fn try_clone(&self) -> Result<Self> {
			let socket = self.socket.try_clone()?;

			Ok(Self { socket })
		}
	};
}

macro_rules! socket_impl {
	($type:ty) => {
		impl Read for $type {
			read_wrapper! {
				inner = socket;
				mut inner = socket;
			}
		}

		impl Write for $type {
			write_wrapper! {
				inner = socket;
				mut inner = socket;
			}
		}

		impl SplitMut for $type {
			type Reader<'a> = SocketHalf<'a>;
			type Writer<'a> = SocketHalf<'a>;

			fn try_split(&mut self) -> Result<(Self::Reader<'_>, Self::Writer<'_>)> {
				let half = SocketHalf::new(self.fd(), self.socket.ready);

				Ok((half, half))
			}
		}
	};
}

pub struct Socket {
	fd: OwnedFd,
	ready: BitFlags<PollFlag>
}

impl_common!(Socket);

#[asynchronous]
impl Socket {
	pub async fn new(
		domain: AddressFamily, socket_type: u32, protocol: IpProtocol
	) -> Result<Self> {
		let fd = io::socket(domain, socket_type, protocol).await?;

		Ok(Self { fd, ready: BitFlags::default() })
	}

	pub async fn new_for_addr(
		addr: &Address, socket_type: u32, protocol: IpProtocol
	) -> Result<Self> {
		match addr {
			Address::V4(_) => Self::new(AddressFamily::INet, socket_type, protocol),
			Address::V6(_) => Self::new(AddressFamily::INet6, socket_type, protocol)
		}
		.await
	}

	#[must_use]
	pub fn fd(&self) -> BorrowedFd<'_> {
		self.fd.as_fd()
	}

	pub async fn close(self) -> Result<()> {
		io::close(self.fd).await
	}

	pub async fn connect(&self, addr: &Address) -> Result<()> {
		io::connect_addr(self.fd(), addr).await
	}

	#[allow(clippy::unused_async)]
	pub async fn set_recvbuf_size(&self, size: i32) -> Result<()> {
		set_recvbuf_size(self.fd(), size).map_err(Into::into)
	}

	#[allow(clippy::unused_async)]
	pub async fn set_sendbuf_size(&self, size: i32) -> Result<()> {
		set_sendbuf_size(self.fd(), size).map_err(Into::into)
	}

	#[allow(clippy::unused_async)]
	pub async fn set_tcp_nodelay(&self, enable: bool) -> Result<()> {
		set_tcp_nodelay(self.fd(), enable).map_err(Into::into)
	}

	#[allow(clippy::unused_async)]
	pub async fn set_tcp_keepalive(&self, enable: bool, idle: i32) -> Result<()> {
		set_tcp_keepalive(self.fd(), enable, idle).map_err(Into::into)
	}

	#[allow(clippy::unused_async)]
	pub async fn local_addr(&self) -> Result<SocketAddr> {
		let mut addr = AddressStorage::default();

		/* Safety: addr is able to store addresses */
		unsafe { get_sock_name(self.fd(), &mut addr)? };

		Ok(convert_addr(addr))
	}

	#[allow(clippy::unused_async)]
	pub async fn peer_addr(&self) -> Result<SocketAddr> {
		let mut addr = AddressStorage::default();

		/* Safety: addr is able to store addresses */
		unsafe { get_peer_name(self.fd(), &mut addr)? };

		Ok(convert_addr(addr))
	}

	#[must_use]
	pub fn half(&self) -> SocketHalf<'_> {
		SocketHalf::new(self.fd(), self.ready)
	}

	pub fn try_clone(&self) -> Result<Self> {
		let fd = self.fd.try_clone()?;

		Ok(Self { fd, ready: self.ready })
	}
}

impl From<OwnedFd> for Socket {
	fn from(fd: OwnedFd) -> Self {
		Self { fd, ready: BitFlags::default() }
	}
}

#[derive(Clone, Copy)]
pub struct SocketHalf<'a> {
	fd: BorrowedFd<'a>,
	ready: BitFlags<PollFlag>
}

impl_common!(SocketHalf<'a>);

#[asynchronous]
impl<'a> SocketHalf<'a> {
	#[must_use]
	pub const fn new(fd: BorrowedFd<'a>, ready: BitFlags<PollFlag>) -> Self {
		Self { fd, ready }
	}

	#[must_use]
	pub const fn fd(&self) -> BorrowedFd<'_> {
		self.fd
	}
}

impl<'a> From<BorrowedFd<'a>> for SocketHalf<'a> {
	fn from(fd: BorrowedFd<'a>) -> Self {
		Self { fd, ready: BitFlags::default() }
	}
}

impl SplitMut for Socket {
	type Reader<'a> = SocketHalf<'a>;
	type Writer<'a> = SocketHalf<'a>;

	fn try_split(&mut self) -> Result<(Self::Reader<'_>, Self::Writer<'_>)> {
		Ok((self.half(), self.half()))
	}
}

pub struct StreamSocket {
	socket: Socket
}

impl StreamSocket {
	socket_common!();

	wrapper_functions! {
		inner = self.socket;

		#[asynchronous]
		pub async fn set_tcp_nodelay(&self, enable: bool) -> Result<()>;

		#[asynchronous]
		pub async fn set_tcp_keepalive(&self, enable: bool, idle: i32) -> Result<()>;
	}
}

socket_impl!(StreamSocket);

pub struct DatagramSocket {
	socket: Socket
}

impl DatagramSocket {
	socket_common!();

	wrapper_functions! {
		inner = self.socket;

		#[asynchronous]
		async fn connect(&self, addr: &Address) -> Result<()>;
	}

	#[asynchronous]
	pub async fn connect_addrs<A>(&self, addrs: A) -> Result<()>
	where
		A: ToSocketAddrs
	{
		foreach_addr(
			addrs,
			|addr| async move { self.socket.connect(&addr).await }
		)
		.await
	}

	#[asynchronous]
	pub async fn recv_from_addr(
		&mut self, from: &SocketAddr, buf: &mut [u8], flags: BitFlags<MessageFlag>
	) -> Result<usize> {
		loop {
			let (read, addr) = self.recvfrom(buf, flags).await?;

			if &addr != from {
				continue;
			}

			break Ok(read);
		}
	}
}

socket_impl!(DatagramSocket);

pub struct TcpListener {
	socket: Socket
}

impl TcpListener {
	wrapper_functions! {
		inner = self.socket;

		#[asynchronous]
		async fn close(self) -> Result<()>;

		#[asynchronous]
		pub async fn local_addr(&self) -> Result<SocketAddr>;

		#[asynchronous]
		pub async fn peer_addr(&self) -> Result<SocketAddr>;
	}

	#[asynchronous]
	pub async fn accept(&self) -> Result<(StreamSocket, SocketAddr)> {
		let mut storage = AddressStorage::default();

		/* Safety: storage is able to store addresses */
		let (fd, _) = unsafe { io::accept(self.socket.fd(), &mut storage).await? };

		Ok((StreamSocket { socket: fd.into() }, convert_addr(storage)))
	}
}

#[allow(missing_copy_implementations)]
pub struct Tcp;

#[asynchronous]
impl Tcp {
	pub async fn connect<A>(addr: A) -> Result<StreamSocket>
	where
		A: ToSocketAddrs
	{
		let sock = connect_addrs(addr, SocketType::Stream as u32, IpProtocol::Tcp).await?;

		Ok(StreamSocket { socket: sock })
	}

	pub async fn bind<A>(addr: A) -> Result<TcpListener>
	where
		A: ToSocketAddrs
	{
		let sock = bind_addr(addr, SocketType::Stream as u32, IpProtocol::Tcp).await?;

		io::listen(sock.fd(), MAX_BACKLOG).await?;

		Ok(TcpListener { socket: sock })
	}
}

#[allow(missing_copy_implementations)]
pub struct Udp;

#[asynchronous]
impl Udp {
	pub async fn connect<A>(addrs: A) -> Result<DatagramSocket>
	where
		A: ToSocketAddrs
	{
		let sock = connect_addrs(addrs, SocketType::Datagram as u32, IpProtocol::Udp).await?;

		Ok(DatagramSocket { socket: sock })
	}

	pub async fn bind<A>(addrs: A) -> Result<DatagramSocket>
	where
		A: ToSocketAddrs
	{
		let sock = bind_addr(addrs, SocketType::Datagram as u32, IpProtocol::Udp).await?;

		Ok(DatagramSocket { socket: sock })
	}
}
