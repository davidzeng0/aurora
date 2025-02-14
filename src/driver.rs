#![allow(unreachable_pub)]

use std::collections::BTreeSet;
use std::os::fd::RawFd;

use enumflags2::BitFlags;
use xx_core::cell::*;
use xx_core::coroutines::{Waker, WakerVTable};
use xx_core::impls::ResultExt;
use xx_core::macros::duration;
use xx_core::opt::hint::*;
use xx_core::os::socket::raw;
use xx_core::os::stat::Statx;
use xx_core::os::time::{self, ClockId};
use xx_core::pointer::*;
use xx_core::threadpool::*;

use super::*;

/// # Safety
/// valid pointer
unsafe fn prepare(ptr: Ptr<()>) {
	let driver = ptr.cast::<Driver>();

	/* Safety: ptr is valid */
	let result = unsafe { ptr!(driver=>io_engine.prepare_wake()) };

	result.expect_nounwind("Fatal error: failed to prepare wake on I/O engine");
}

/// # Safety
/// valid pointer
///
/// See [`Request::complete`]
unsafe fn wake(ptr: Ptr<()>, request: ReqPtr<()>) {
	let driver = ptr.cast::<Driver>();

	/* Safety: this function call is thread safe */
	let result = unsafe { ptr!(driver=>io_engine.wake(request)) };

	result.expect_nounwind("Fatal error: failed to wake I/O engine");
}

static WAKER: WakerVTable = unsafe { WakerVTable::new(prepare, wake) };

fn shutdown() -> Error {
	fmt_error!("Driver is shutting down" @ ErrorKind::Shutdown)
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Timeout {
	expire: u64,
	request: ReqPtr<Result<()>>
}

pub struct Driver {
	timers: UnsafeCell<BTreeSet<Timeout>>,
	exiting: Cell<bool>,
	io_engine: Engine
}

impl Driver {
	pub fn new() -> Result<Self> {
		Ok(Self {
			timers: UnsafeCell::new(BTreeSet::new()),
			exiting: Cell::new(false),
			io_engine: Engine::new()?
		})
	}

	#[inline(always)]
	fn time() -> u64 {
		time::nanotime(ClockId::Monotonic).expect_nounwind("Failed to read the clock")
	}

	/// # Safety
	/// See [`Request::complete`]
	unsafe fn timer_complete(timeout: Timeout, result: Result<()>) {
		/* Safety: guaranteed by caller */
		unsafe { Request::complete(timeout.request, result) };
	}

	fn queue_timer(&self, timer: Timeout) {
		/* Safety: exclusive unsafe cell access */
		unsafe { ptr!(self.timers=>insert(timer)) };
	}

	fn cancel_timer(&self, timer: Timeout) -> Result<()> {
		/* Safety: we have exclusive mutable access until expire */
		let timeout = match unsafe { ptr!(self.timers=>take(&timer)) } {
			Some(timeout) => timeout,
			None => return Err(fmt_error!("Timer not found" @ ErrorKind::NotFound))
		};

		xx_core::trace!(target: self, "## cancel_timer(request = {:?}) = Ok(reason = cancel)", timeout.request);

		/* Safety: complete the future */
		unsafe {
			Self::timer_complete(
				timeout,
				Err(fmt_error!("Timer cancelled" @ ErrorKind::Interrupted))
			);
		}

		Ok(())
	}

	#[future]
	pub fn timeout(&self, mut expire: u64, flags: BitFlags<TimeoutFlag>, request: _) -> Result<()> {
		#[cancel]
		fn cancel(&self, expire: u64) -> Result<()> {
			self.cancel_timer(Timeout { expire, request })
		}

		if let Err(err) = self.check_exiting() {
			return Progress::Done(Err(err));
		}

		#[allow(clippy::expect_used)]
		if !flags.intersects(TimeoutFlag::Abs) {
			expire = expire.checked_add(nanotime()).expect("Timeout overflow");
		}

		xx_core::trace!(target: self, "## timeout(expire = {}, request = {:?}) = Ok(())", expire, request);

		self.queue_timer(Timeout { expire, request });

		Progress::Pending(cancel(self, expire))
	}

	#[allow(clippy::missing_panics_doc)]
	fn run_timers(&self) -> u64 {
		#[allow(clippy::cast_possible_truncation)]
		let mut timeout = duration!(1 hour).as_nanos() as u64;
		let mut now = Self::time();
		let mut ran = false;

		loop {
			/* Safety: we have mutable access until expire */
			let timers = unsafe { &mut ptr!(*self.timers) };
			let timer = match timers.first() {
				None => break,
				Some(timer) => timer
			};

			if timer.expire > now {
				if ran {
					now = Self::time();
				}

				timeout = timer.expire.saturating_sub(now);

				break;
			}

			ran = true;

			xx_core::trace!(target: self, "## run_timers: complete(request = {:?}, reason = timeout)", timer.request);

			#[allow(clippy::unwrap_used)]
			let timer = timers.pop_first().unwrap();

			/* Safety: complete the future */
			unsafe { Self::timer_complete(timer, Ok(())) };
		}

		timeout
	}

	fn park(&self, timeout: u64) {
		self.io_engine
			.work(timeout)
			.expect_nounwind("Fatal error from engine");
	}

	pub fn block_while<F>(&self, block: F)
	where
		F: Fn() -> bool
	{
		loop {
			let timeout = self.run_timers();

			if unlikely(!block()) {
				break;
			}

			self.park(timeout);

			if unlikely(!block()) {
				break;
			}
		}
	}

	#[allow(clippy::missing_panics_doc)]
	pub fn exit(&self) {
		self.exiting.set(true);

		loop {
			/* Safety: we have exclusive access until expire */
			let timers = unsafe { &mut ptr!(*self.timers) };

			if timers.is_empty() {
				break;
			}

			#[allow(clippy::unwrap_used)]
			let timeout = timers.pop_first().unwrap();

			/* Safety: complete the future */
			unsafe { Self::timer_complete(timeout, Err(shutdown())) };
		}

		loop {
			let timeout = self.run_timers();

			if !self.io_engine.has_work() {
				break;
			}

			self.park(timeout);
		}

		self.exiting.set(false);
	}

	#[inline(always)]
	pub fn check_exiting(&self) -> Result<()> {
		if likely(!self.exiting.get()) {
			Ok(())
		} else {
			Err(shutdown())
		}
	}

	pub fn waker(&self) -> Waker {
		Waker::new(ptr!(self).cast(), &WAKER)
	}
}

macro_rules! engine_task {
	($func: ident ($($arg: ident: $type: ty),*)) => {
		/// # Safety
		/// See [`Future::run`]
		#[future]
		pub unsafe fn $func(&self, $($arg: $type),*, request: _) -> isize {
			#[cancel]
			fn cancel(engine: &Engine) -> Result<()> {
				/* use this fn to generate the cancel closure type */
				Ok(())
			}

			#[allow(clippy::multiple_unsafe_ops_per_block)]
			/* Safety: guaranteed by caller */
			unsafe { self.io_engine.$func($($arg),*).run(request) }
		}
	}
}

impl Driver {
	engine_task!(open(path: Ptr<()>, flags: u32, mode: u32));

	engine_task!(close(fd: RawFd));

	engine_task!(read(fd: RawFd, buf: MutPtr<()>, len: usize, offset: i64));

	engine_task!(write(fd: RawFd, buf: Ptr<()>, len: usize, offset: i64));

	engine_task!(socket(domain: u32, sockettype: u32, protocol: u32));

	engine_task!(accept(socket: RawFd, addr: MutPtr<()>, addrlen: MutPtr<i32>));

	engine_task!(connect(socket: RawFd, addr: Ptr<()>, addrlen: i32));

	engine_task!(recv(socket: RawFd, buf: MutPtr<()>, len: usize, flags: u32));

	engine_task!(recvmsg(socket: RawFd, header: MutPtr<raw::MsgHdr>, flags: u32));

	engine_task!(send(socket: RawFd, buf: Ptr<()>, len: usize, flags: u32));

	engine_task!(sendmsg(socket: RawFd, header: Ptr<raw::MsgHdr>, flags: u32));

	engine_task!(shutdown(socket: RawFd, how: u32));

	engine_task!(bind(socket: RawFd, addr: Ptr<()>, addrlen: i32));

	engine_task!(listen(socket: RawFd, backlog: i32));

	engine_task!(fsync(file: RawFd));

	engine_task!(statx(dirfd: RawFd, path: Ptr<()>, flags: u32, mask: u32, statx: MutPtr<Statx>));

	engine_task!(poll(fd: RawFd, mask: u32));

	#[future]
	pub unsafe fn run_work(&self, work: MutPtr<Work<'_>>, request: _) -> bool {
		#[cancel]
		fn cancel(engine: &Engine, cancel: CancelWork) -> Result<()> {
			/* use this fn to generate the cancel closure type */
			Ok(())
		}

		#[allow(clippy::multiple_unsafe_ops_per_block)]
		/* Safety: guaranteed by caller */
		unsafe {
			self.io_engine.run_work(work).run(request)
		}
	}
}

impl Pin for Driver {
	unsafe fn pin(&mut self) {
		/* Safety: we are being pinned */
		unsafe { self.io_engine.pin() };
	}
}
