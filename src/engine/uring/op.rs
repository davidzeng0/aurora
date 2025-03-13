#![allow(
	clippy::unwrap_used,
	clippy::cast_sign_loss,
	clippy::cast_possible_wrap
)]

use xx_core::macros::const_assert;
use xx_core::os::epoll::*;
use xx_core::os::iovec::raw::*;
use xx_core::os::openat2::OpenHow;

use super::*;

fn new_op(op: OpCode) -> SubmissionEntry {
	let mut entry = SubmissionEntry::default();

	entry.op = op;
	entry
}

fn rw(entry: &mut SubmissionEntry, fd: i32, addr: u64, len: u32, off: u64, flags: u32) {
	entry.fd = fd;
	entry.addr.addr = addr;
	entry.len = len;
	entry.off.off = off;
	entry.rw_flags = flags;
}

fn rw_fixed(
	entry: &mut SubmissionEntry, fd: i32, addr: u64, len: u32, off: u64, flags: u32, buf_index: u16
) {
	rw(entry, fd, addr, len, off, flags);

	entry.buf = buf_index;
}

fn close(entry: &mut SubmissionEntry, fd: i32, file_index: u32) {
	rw_fixed(entry, fd, 0, 0, 0, 0, 0);

	entry.file.file_index = file_index;
}

fn sync(entry: &mut SubmissionEntry, fd: i32, len: u32, off: u64, flags: u32) {
	rw_fixed(entry, fd, 0, len, off, flags, 0);

	entry.file.splice_fd_in = 0;
}

fn advise(entry: &mut SubmissionEntry, addr: u64, len: u32, off: u64, flags: u32) {
	entry.addr.addr = addr;
	entry.len = len;
	entry.off.off = off;
	entry.rw_flags = flags;
	entry.buf = 0;
	entry.file.splice_fd_in = 0;
}

fn fs(entry: &mut SubmissionEntry, fd0: i32, ptr0: u64, fd1: i32, ptr1: u64, flags: u32) {
	entry.fd = fd0;
	entry.addr.addr = ptr0;
	entry.len = fd1 as u32;
	entry.off.off = ptr1;
	entry.rw_flags = flags;
	entry.buf = 0;
	entry.file.splice_fd_in = 0;
}

fn fxattr(
	entry: &mut SubmissionEntry, fd: i32, name: Ptr<()>, value: MutPtr<()>, len: u32, flags: u32
) {
	entry.fd = fd;
	entry.addr.addr = name.addr() as u64;
	entry.len = len;
	entry.off.addr = value.addr() as u64;
	entry.rw_flags = flags;
}

fn xattr(
	entry: &mut SubmissionEntry, path: Ptr<()>, name: Ptr<()>, value: MutPtr<()>, len: u32,
	flags: u32
) {
	entry.addr3.addr = path.addr() as u64;
	entry.addr.addr = name.addr() as u64;
	entry.len = len;
	entry.off.addr = value.addr() as u64;
	entry.rw_flags = flags;
}

fn splice(
	entry: &mut SubmissionEntry, fd_in: i32, off_in: u64, fd_out: i32, off_out: u64, len: u32,
	flags: u32
) {
	entry.file.splice_fd_in = fd_in;
	entry.addr.off = off_in;
	entry.fd = fd_out;
	entry.off.off = off_out;
	entry.len = len;
	entry.rw_flags = flags;
}

fn socket(
	entry: &mut SubmissionEntry, fd: i32, addr: u64, len: u32, off: u64, flags: u32,
	file_index: u32
) {
	rw_fixed(entry, fd, addr, len, off, flags, 0);

	entry.file.file_index = file_index;
}

fn socket_rw(entry: &mut SubmissionEntry, fd: i32, addr: u64, len: u32, flags: u32) {
	rw(entry, fd, addr, len, 0, flags);

	entry.file.file_index = 0;
}

fn buffer(entry: &mut SubmissionEntry, addr: usize, len: u32, nr: u16, bgid: u16, bid: u16) {
	entry.fd = nr as i32;
	entry.addr.addr = addr as u64;
	entry.len = len;
	entry.off.off = bid as u64;
	entry.buf = bgid;
	entry.rw_flags = 0;
}

#[cfg(target_endian = "little")]
const fn swap_poll_mask(mask: u32) -> u32 {
	mask
}

#[cfg(target_endian = "big")]
const fn swap_poll_mask(mask: u32) -> u32 {
	(mask << 16) | (mask >> 16)
}

pub struct Op;

#[allow(dead_code)]
impl Op {
	pub fn nop() -> SubmissionEntry {
		new_op(OpCode::NoOp)
	}

	pub fn openat(
		dfd: i32, path: Ptr<()>, flags: u32, mode: u32, file_index: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::OpenAt);

		entry.fd = dfd;
		entry.addr.addr = path.addr() as u64;
		entry.len = mode;
		entry.rw_flags = flags;
		entry.buf = 0;
		entry.file.file_index = file_index;
		entry
	}

	#[allow(clippy::cast_possible_truncation)]
	pub fn openat2(
		dfd: i32, path: Ptr<()>, how: MutPtr<OpenHow>, file_index: u32
	) -> SubmissionEntry {
		const_assert!(size_of::<OpenHow>() <= u32::MAX as usize);

		let mut entry = new_op(OpCode::OpenAt2);

		entry.fd = dfd;
		entry.addr.addr = path.addr() as u64;
		entry.len = size_of::<OpenHow>() as u32;
		entry.off.addr = how.addr() as u64;
		entry.buf = 0;
		entry.file.file_index = file_index;
		entry
	}

	pub fn close(fd: i32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Close);

		close(&mut entry, fd, 0);

		entry
	}

	pub fn close_direct(file_index: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Close);

		close(&mut entry, 0, file_index);

		entry
	}

	pub fn read(fd: i32, addr: MutPtr<()>, len: u32, off: i64, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Read);

		rw(&mut entry, fd, addr.addr() as u64, len, off as u64, flags);

		entry
	}

	pub fn write(fd: i32, addr: Ptr<()>, len: u32, off: i64, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Write);

		rw(&mut entry, fd, addr.addr() as u64, len, off as u64, flags);

		entry
	}

	pub fn readv(
		fd: i32, iovecs: MutPtr<IoVec>, iovecs_len: u32, off: i64, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::ReadVector);

		rw(
			&mut entry,
			fd,
			iovecs.addr() as u64,
			iovecs_len,
			off as u64,
			flags
		);

		entry
	}

	pub fn writev(
		fd: i32, iovecs: Ptr<IoVec>, iovecs_len: u32, off: i64, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::WriteVector);

		rw(
			&mut entry,
			fd,
			iovecs.addr() as u64,
			iovecs_len,
			off as u64,
			flags
		);

		entry
	}

	pub fn read_fixed(
		fd: i32, addr: MutPtr<()>, len: u32, off: i64, buf_index: u16, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::ReadFixed);

		rw_fixed(
			&mut entry,
			fd,
			addr.addr() as u64,
			len,
			off as u64,
			flags,
			buf_index
		);

		entry
	}

	pub fn write_fixed(
		fd: i32, addr: Ptr<()>, len: u32, off: i64, buf_index: u16, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::WriteFixed);

		rw_fixed(
			&mut entry,
			fd,
			addr.addr() as u64,
			len,
			off as u64,
			flags,
			buf_index
		);

		entry
	}

	pub fn fsync(fd: i32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::FileSync);

		sync(&mut entry, fd, 0, 0, flags);

		entry
	}

	pub fn timeout(timespec: Ptr<TimeSpec>, count: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Timeout);

		rw(
			&mut entry,
			0,
			ptr!(timespec).addr() as u64,
			1,
			count as u64,
			flags
		);

		entry
	}

	pub fn sync_file_range(fd: i32, len: u32, off: i64, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::SyncFileRange);

		sync(&mut entry, fd, len, off as u64, flags);

		entry
	}

	pub fn fallocate(fd: i32, mode: i32, off: i64, len: i64) -> SubmissionEntry {
		let mut entry = new_op(OpCode::FileAllocate);

		rw_fixed(&mut entry, fd, len as u64, mode as u32, off as u64, 0, 0);

		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn fadvise(fd: i32, off: u64, len: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::FileAdvise);

		advise(&mut entry, 0, len, off, flags);

		entry.fd = fd;
		entry
	}

	pub fn madvise(addr: Ptr<()>, len: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::MemoryAdvise);

		advise(&mut entry, addr.addr() as u64, len, 0, flags);

		entry
	}

	pub fn renameat(
		old_dfd: i32, old_path: Ptr<()>, new_dfd: i32, new_path: Ptr<()>, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::RenameAt);

		fs(
			&mut entry,
			old_dfd,
			old_path.addr() as u64,
			new_dfd,
			new_path.addr() as u64,
			flags
		);

		entry
	}

	pub fn unlinkat(dfd: i32, path: Ptr<()>, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::UnlinkAt);

		fs(&mut entry, dfd, path.addr() as u64, 0, 0, flags);

		entry
	}

	pub fn mkdirat(dfd: i32, path: Ptr<()>, mode: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::MkdirAt);

		fs(&mut entry, dfd, path.addr() as u64, mode as i32, 0, 0);

		entry
	}

	pub fn symlinkat(target: Ptr<()>, newdirfd: i32, linkpath: Ptr<()>) -> SubmissionEntry {
		let mut entry = new_op(OpCode::SymlinkAt);

		fs(
			&mut entry,
			newdirfd,
			target.addr() as u64,
			0,
			linkpath.addr() as u64,
			0
		);

		entry
	}

	pub fn linkat(
		old_dfd: i32, old_path: Ptr<()>, new_dfd: i32, new_path: Ptr<()>, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::LinkAt);

		fs(
			&mut entry,
			old_dfd,
			old_path.addr() as u64,
			new_dfd,
			new_path.addr() as u64,
			flags
		);

		entry
	}

	pub fn fgetxattr(fd: i32, name: Ptr<()>, value: MutPtr<()>, len: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::FileGetXAttr);

		fxattr(&mut entry, fd, name, value, len, 0);

		entry
	}

	pub fn fsetxattr(
		fd: i32, name: Ptr<()>, value: Ptr<()>, len: u32, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::FileSetXAttr);

		fxattr(&mut entry, fd, name, value.cast_mut(), len, flags);

		entry
	}

	pub fn getxattr(path: Ptr<()>, name: Ptr<()>, value: MutPtr<()>, len: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::GetXAttr);

		xattr(&mut entry, path, name, value, len, 0);

		entry
	}

	pub fn setxattr(
		path: Ptr<()>, name: Ptr<()>, value: Ptr<()>, len: u32, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::SetXAttr);

		xattr(&mut entry, path, name, value.cast_mut(), len, flags);

		entry
	}

	pub fn splice(
		fd_in: i32, off_in: i64, fd_out: i32, off_out: i64, len: u32, flags: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Splice);

		splice(
			&mut entry,
			fd_in,
			off_in as u64,
			fd_out,
			off_out as u64,
			len,
			flags
		);

		entry
	}

	pub fn tee(fd_in: i32, fd_out: i32, len: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Tee);

		splice(&mut entry, fd_in, 0, fd_out, 0, len, flags);

		entry
	}

	pub fn statx(
		fd: i32, path: Ptr<()>, flags: u32, mask: u32, statx: MutPtr<Statx>
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Statx);

		rw_fixed(
			&mut entry,
			fd,
			path.addr() as u64,
			mask,
			statx.addr() as u64,
			flags,
			0
		);

		entry
	}

	pub fn socket(
		domain: u32, socket_type: u32, protocol: u32, flags: u32, file_index: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Socket);

		socket(
			&mut entry,
			domain as i32,
			0,
			protocol,
			socket_type as u64,
			flags,
			file_index
		);

		entry
	}

	pub fn connect(fd: i32, addr: Ptr<()>, addrlen: i32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Connect);

		socket(&mut entry, fd, addr.addr() as u64, 0, addrlen as u64, 0, 0);

		entry
	}

	pub fn accept(
		fd: i32, addr: MutPtr<()>, addrlen: MutPtr<i32>, flags: u32, file_index: u32
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Accept);

		socket(
			&mut entry,
			fd,
			addr.addr() as u64,
			0,
			addrlen.addr() as u64,
			flags,
			file_index
		);

		entry
	}

	pub fn recv(fd: i32, buf: MutPtr<()>, len: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Recv);

		socket_rw(&mut entry, fd, buf.addr() as u64, len, flags);

		entry
	}

	pub fn send(fd: i32, buf: Ptr<()>, len: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Send);

		socket_rw(&mut entry, fd, buf.addr() as u64, len, flags);

		entry
	}

	pub fn recvmsg(fd: i32, msg: MutPtr<MsgHdr>, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::RecvMsg);

		socket_rw(&mut entry, fd, msg.addr() as u64, 1, flags);

		entry
	}

	pub fn sendmsg(fd: i32, msg: Ptr<MsgHdr>, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::SendMsg);

		socket_rw(&mut entry, fd, msg.addr() as u64, 1, flags);

		entry
	}

	pub fn send_zc(fd: i32, buf: usize, len: u32, flags: u32, buf_index: u16) -> SubmissionEntry {
		Self::sendto_zc(fd, buf, len, flags, Ptr::null(), 0, buf_index)
	}

	/// # Panics
	/// if `addrlen` is larger than `u16::MAX`
	pub fn sendto_zc(
		fd: i32, buf: usize, len: u32, flags: u32, addr: Ptr<()>, addrlen: u32, buf_index: u16
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::SendZeroCopy);

		entry.fd = fd;
		entry.addr.addr = buf as u64;
		entry.len = len;
		entry.off.addr = addr.addr() as u64;
		entry.file.addr_len.len = addrlen.try_into().unwrap();
		entry.file.addr_len.pad = [0u16; 1];
		entry.rw_flags = flags;
		entry.addr3.addr = 0;
		entry.buf = buf_index;
		entry.pad = [0u64; 1];
		entry
	}

	pub fn shutdown(fd: i32, how: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::Shutdown);

		socket(&mut entry, fd, 0, how, 0, 0, 0);

		entry
	}

	pub fn poll(fd: i32, mask: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::PollAdd);

		rw_fixed(&mut entry, fd, 0, 0, 0, swap_poll_mask(mask), 0);

		entry
	}

	pub fn poll_update(mask: u32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::PollRemove);

		entry.len = flags;
		entry.off.addr = 0;
		entry.rw_flags = swap_poll_mask(mask);
		entry.buf = 0;
		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn epoll_ctl(ep: i32, op: u32, fd: i32, event: MutPtr<Event>) -> SubmissionEntry {
		let mut entry = new_op(OpCode::EPollCtl);

		entry.buf = 0;
		entry.file.splice_fd_in = 0;
		entry.fd = ep;
		entry.addr.addr = event.addr() as u64;
		entry.len = op;
		entry.off.addr = fd as u64;
		entry
	}

	pub fn poll_cancel() -> SubmissionEntry {
		let mut entry = new_op(OpCode::PollRemove);

		entry.len = 0;
		entry.off.addr = 0;
		entry.rw_flags = 0;
		entry.buf = 0;
		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn cancel(flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::AsyncCancel);

		entry.len = 0;
		entry.off.addr = 0;
		entry.rw_flags = flags;
		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn cancel_fd(fd: i32, flags: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::AsyncCancel);

		rw(&mut entry, fd, 0, 0, 0, flags | AsyncCancelFlag::Fd as u32);

		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn cancel_fixed(file_index: u32, flags: u32) -> SubmissionEntry {
		Self::cancel_fd(file_index as i32, flags | AsyncCancelFlag::FdFixed as u32)
	}

	pub fn cancel_all() -> SubmissionEntry {
		let mut entry = new_op(OpCode::AsyncCancel);

		rw(&mut entry, 0, 0, 0, 0, AsyncCancelFlag::Any as u32);

		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn files_update(fds: MutPtr<i32>, len: u32, off: u32) -> SubmissionEntry {
		let mut entry = new_op(OpCode::FilesUpdate);

		entry.addr.addr = fds.addr() as u64;
		entry.len = len;
		entry.off.off = off as u64;
		entry.rw_flags = 0;
		entry.file.splice_fd_in = 0;
		entry
	}

	pub fn provide_buffers(
		addr: Ptr<()>, len: u32, count: u16, bgid: u16, bid: u16
	) -> SubmissionEntry {
		let mut entry = new_op(OpCode::ProvideBuffers);

		buffer(&mut entry, addr.addr(), len, count, bgid, bid);

		entry
	}

	pub fn remove_buffers(count: u16, bgid: u16) -> SubmissionEntry {
		let mut entry = new_op(OpCode::RemoveBuffers);

		buffer(&mut entry, 0, 0, count, bgid, 0);

		entry
	}
}
