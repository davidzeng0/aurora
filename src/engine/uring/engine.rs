#![allow(clippy::multiple_unsafe_ops_per_block)]

use std::collections::VecDeque;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::sync::atomic::{compiler_fence, AtomicU32, Ordering};
use std::sync::Mutex;

use enumflags2::{make_bitflags, BitFlags};
use xx_core::cell::Cell;
use xx_core::impls::ResultExt;
use xx_core::macros::{assert_unsafe_precondition, panic_nounwind};
use xx_core::opt::hint::*;
use xx_core::os::error::*;
use xx_core::os::eventfd::*;
use xx_core::os::mman::*;
use xx_core::os::openat::*;
use xx_core::os::poll::PollFlag;
use xx_core::threadpool::*;
use xx_core::{debug, error, trace, warn};

use super::*;

struct Rings<'mem> {
	ring: Map<'mem>,
	separate_completion_ring: Option<Map<'mem>>,
	submission_entries: Map<'mem>
}

impl<'mem> Rings<'mem> {
	#[allow(clippy::arithmetic_side_effects)]
	const fn scale<T>(mut count: u32, offset: u32, wide: bool) -> usize {
		if wide {
			count *= 2;
		}

		offset as usize + size_of::<T>() * count as usize
	}

	fn map_memory(size: usize, offset: MmapOffsets, fd: BorrowedFd<'_>) -> OsResult<Map<'mem>> {
		Builder::new(Type::Shared, size)
			.protect(Protection::Read | Protection::Write)
			.flag(Flag::Populate)
			.fd(fd)
			.offset(offset as isize)
			.map()
	}

	#[allow(clippy::missing_const_for_fn)]
	fn submission_ring(&self) -> &Map<'mem> {
		&self.ring
	}

	fn completion_ring(&self) -> &Map<'mem> {
		self.separate_completion_ring.as_ref().unwrap_or(&self.ring)
	}

	fn new(fd: BorrowedFd<'_>, params: &Parameters) -> Result<Self> {
		let ring_sizes = (
			Self::scale::<u32>(params.sq_entries, params.sq_off.array, false),
			Self::scale::<CompletionEntry>(
				params.cq_entries,
				params.cq_off.cqes,
				params.flags().intersects(SetupFlag::CompletionEntryWide)
			)
		);

		let (ring, separate_completion_ring) = if params.features().intersects(Feature::SingleMmap)
		{
			(
				Self::map_memory(
					ring_sizes.0.max(ring_sizes.1),
					MmapOffsets::SubmissionRing,
					fd
				)?,
				None
			)
		} else {
			(
				Self::map_memory(ring_sizes.0, MmapOffsets::SubmissionRing, fd)?,
				Some(Self::map_memory(
					ring_sizes.1,
					MmapOffsets::CompletionRing,
					fd
				)?)
			)
		};

		let submission_entries_size = Self::scale::<SubmissionEntry>(
			params.sq_entries,
			0,
			params.flags().intersects(SetupFlag::SubmissionEntryWide)
		);

		Ok(Self {
			ring,
			separate_completion_ring,
			submission_entries: Self::map_memory(
				submission_entries_size,
				MmapOffsets::SubmissionEntries,
				fd
			)?
		})
	}
}

#[allow(dead_code)]
#[repr(C)]
struct SubmissionQueue<'mem> {
	kflags: &'mem AtomicU32,
	ktail: &'mem AtomicU32,

	tail: Cell<u32>,
	mask: u32,
	entries: MutPtr<[SubmissionEntry]>,

	capacity: u32,

	/* unused */
	khead: &'mem AtomicU32,
	kdropped: &'mem AtomicU32,
	array: MutPtr<[u32]>
}

#[allow(dead_code)]
#[repr(C)]
struct CompletionQueue<'mem> {
	khead: &'mem AtomicU32,
	ktail: &'mem AtomicU32,
	entries: MutPtr<[CompletionEntry]>,
	mask: u32,

	/* unused */
	kflags: &'mem AtomicU32,
	koverflow: &'mem AtomicU32,
	capacity: u32
}

#[allow(dead_code)]
#[repr(C)]
struct Queue {
	submission: SubmissionQueue<'static>,
	rings: Rings<'static>,
	completion: CompletionQueue<'static>
}

/// # Safety
/// valid offset
const unsafe fn get_ptr<T>(map: &Map<'_>, off: u32) -> MutPtr<T> {
	/* Safety: guaranteed by caller */
	unsafe { map.as_ptr().cast::<u8>().add(off as usize).cast() }
}

/// # Safety
/// the offset must be valid for the type T
unsafe fn get_ref<'mem, T>(map: &Map<'mem>, off: u32) -> &'mem T {
	/* Safety: guaranteed by caller */
	unsafe { get_ptr::<T>(map, off).as_ref() }
}

/// # Safety
/// valid offset and len
unsafe fn get_array<T>(map: &Map<'_>, off: u32, len: u32) -> MutPtr<[T]> {
	/* Safety: guaranteed by caller */
	let base = unsafe { get_ptr::<T>(map, off) };

	MutPtr::slice_from_raw_parts(base, len as usize)
}

impl<'mem> SubmissionQueue<'mem> {
	/// # Safety
	/// `params` must be initialized by a successfull call to io_uring_setup
	#[allow(unsafe_op_in_unsafe_fn)]
	unsafe fn new(maps: &Rings<'mem>, params: &Parameters) -> Self {
		let ring = maps.submission_ring();
		let sq = SubmissionQueue {
			khead: get_ref(ring, params.sq_off.head),
			ktail: get_ref(ring, params.sq_off.tail),
			kflags: get_ref(ring, params.sq_off.flags),
			kdropped: get_ref(ring, params.sq_off.dropped),

			array: get_array(ring, params.sq_off.array, params.sq_entries),
			entries: get_array(&maps.submission_entries, 0, params.sq_entries),

			#[allow(clippy::arithmetic_side_effects)]
			mask: params.sq_entries - 1,
			capacity: params.sq_entries,

			tail: Cell::new(0)
		};

		/* Safety: valid array */
		for (i, elem) in unsafe { sq.array.into_iter() }.enumerate() {
			/* Safety: set index */
			#[allow(clippy::cast_possible_truncation)]
			(unsafe { ptr!(*elem) = i as u32 });
		}

		sq
	}

	fn flags(&self) -> BitFlags<SubmissionRingFlag> {
		let flags = self.kflags.load(Ordering::Relaxed);

		BitFlags::from_bits_truncate(flags)
	}

	/// # Safety
	/// index must be in range
	unsafe fn write(&self, index: u32, entry: SubmissionEntry) {
		/* Safety: guaranteed by caller */
		unsafe { assert_unsafe_precondition!((index as usize) < self.entries.len()) };

		/* Safety: guaranteed by caller */
		unsafe { ptr!(self.entries=>[index as usize] = entry) };
	}

	fn push(&self, entry: SubmissionEntry) {
		let tail = self.tail.get();

		#[allow(clippy::arithmetic_side_effects)]
		self.tail.update(|tail| tail + 1);

		/* Safety: tail is masked */
		unsafe { self.write(tail & self.mask, entry) };
	}

	fn sync(&self) {
		self.ktail.store(self.tail.get(), Ordering::Release);
	}
}

#[allow(dead_code)]
impl<'mem> CompletionQueue<'mem> {
	/// # Safety
	/// `params` must be initialized by a successfull call to io_uring_setup
	#[allow(unsafe_op_in_unsafe_fn)]
	unsafe fn new(maps: &Rings<'mem>, params: &Parameters) -> Self {
		let ring = maps.completion_ring();

		CompletionQueue {
			khead: get_ref(ring, params.cq_off.head),
			ktail: get_ref(ring, params.cq_off.tail),
			kflags: get_ref(ring, params.cq_off.flags),
			koverflow: get_ref(ring, params.cq_off.overflow),

			entries: get_array(ring, params.cq_off.cqes, params.cq_entries),

			#[allow(clippy::arithmetic_side_effects)]
			mask: params.cq_entries - 1,
			capacity: params.cq_entries
		}
	}

	fn flags(&self) -> BitFlags<CompletionRingFlag> {
		let flags = self.kflags.load(Ordering::Relaxed);

		BitFlags::from_bits_truncate(flags)
	}

	/// # Safety
	/// index must be in bounds
	unsafe fn read(&self, index: u32) -> CompletionEntry {
		/* Safety: guaranteed by caller */
		unsafe { assert_unsafe_precondition!((index as usize) < self.entries.len()) };

		/* Safety: guaranteed by caller */
		unsafe { ptr!(self.entries=>[index as usize]) }
	}

	fn read_ring(&self) -> (u32, u32) {
		let result = (
			self.khead.load(Ordering::Relaxed),
			self.ktail.load(Ordering::Relaxed)
		);

		compiler_fence(Ordering::Acquire);

		result
	}
}

impl Queue {
	/// # Safety
	/// `params` must be initialized by a successfull call to io_uring_setup
	#[allow(unsafe_op_in_unsafe_fn)]
	unsafe fn new(rings: Rings<'static>, params: Parameters) -> Self {
		Self {
			submission: SubmissionQueue::new(&rings, &params),
			completion: CompletionQueue::new(&rings, &params),
			rings
		}
	}

	fn needs_flush(&self) -> bool {
		self.submission
			.flags()
			.intersects(SubmissionRingFlag::CqOverflow)
	}

	fn needs_enter(&self) -> bool {
		self.submission
			.flags()
			.intersects(SubmissionRingFlag::CqOverflow | SubmissionRingFlag::TaskRun)
	}
}

fn create_io_uring() -> Result<(IoRingFeatures, OwnedFd, Parameters)> {
	struct IoUringSetup {}

	let ring = IoUringSetup {};

	let features = match io_uring_detect_features()? {
		Some(features) => features,
		None => {
			error!(target: &ring,
				"== Failed to setup io_uring.\n\
				:: The current linux kernel does not support io_uring.\n\
				:: Please upgrade the kernel to a minimum of version 5.11 (recommended >= 6.1) and try again."
			);

			return Err(OsError::NoSys.into());
		}
	};

	let ops = [
		(OpCode::OpenAt, Some("files"), None),
		(OpCode::Close, None, Some("Some operations may be blocking")),
		(OpCode::Read, Some("files"), None),
		(OpCode::Write, Some("files"), None),
		(OpCode::Socket, None, Some("Using syscall as fallback")),
		(OpCode::Accept, Some("sockets"), None),
		(OpCode::Connect, Some("sockets"), None),
		(OpCode::Recv, Some("sockets"), None),
		(OpCode::RecvMsg, Some("sockets"), None),
		(OpCode::Send, Some("sockets"), None),
		(OpCode::SendMsg, Some("sockets"), None),
		(
			OpCode::Shutdown,
			None,
			Some("Some operations may be blocking")
		),
		(OpCode::FileSync, Some("files"), None),
		(OpCode::Statx, Some("files"), None),
		(OpCode::PollAdd, None, None)
	];

	for (op, feature, message) in &ops {
		if !features.opcode_supported(*op) {
			let feature = feature
				.map(|feat| format!("(like {})", feat))
				.unwrap_or(String::new());

			warn!(
				target: &ring,
				"== Op code `{:?}`, is not supported.\n\
				:: {}.\n\
				:: Linux kernel version 6.1 or greater is recommended.",
				*op,
				message.unwrap_or(&format!(
					"Some features {} may not be available",
					feature
				))
			);
		}
	}

	let mut setup_flags = BitFlags::default();
	let mut params = Parameters::default();

	let flags = make_bitflags!(SetupFlag::{
		CompletionRingSize | Clamp | SubmitAll | CoopTaskrun | TaskrunFlag | SingleIssuer | DeferTaskrun
	});

	for flag in flags {
		if features.setup_flag_supported(flag) {
			setup_flags |= flag;
		}
	}

	params.sq_entries = 0x100;
	params.cq_entries = 0x2000;
	params.set_flags(setup_flags);

	if !setup_flags.intersects(SetupFlag::Clamp) {
		params.cq_entries = 0;
	}

	if !setup_flags.contains(flags) || !features.feature_supported(Feature::ExtArg) {
		warn!(
			target: &ring,
			"== Running in compatibility mode on an estimated linux kernel version of {}.\n\
			:: The preferred version is atleast 6.1. Some features may not be available.\n\
			:: Performance may be degraded.",
			features.version()
		);
	}

	match io_uring_setup(params.sq_entries, &mut params) {
		Ok(fd) => {
			debug!(
				target: &ring,
				"++ Initialized with {}:{} entries",
				params.sq_entries,
				params.cq_entries
			);

			Ok((features, fd, params))
		}

		Err(err) => {
			if err == OsError::NoMem {
				error!(target: &ring,
					"== Failed to setup io_uring.\n\
					:: This is usually because the current locked memory limit is too low.\n\
					:: Please raise the limit and try again."
				);
			}

			Err(err.into())
		}
	}
}

#[repr(C)]
pub struct IoUring {
	ring_fd: OwnedFd,

	to_submit: Cell<u32>,
	queue: Queue,
	to_complete: Cell<u64>,

	features: IoRingFeatures,

	expected_wakes: Cell<usize>,
	wake_queue: Mutex<VecDeque<ReqPtr<()>>>,

	event_fd: EventFd,
	event_request: Request<isize>,

	thread_pool: ThreadPool
}

static NO_OP: Request<isize> = Request::no_op();

impl IoUring {
	#[cold]
	#[inline(never)]
	fn process_wake_cold(&self, events: isize) {
		let events = Engine::result_for_poll(events).expect_nounwind("Failed to poll event fd");
		let flags = BitFlags::from_bits_truncate(events);

		if flags.intersects(PollFlag::Error) {
			panic_nounwind!("Error flag on event fd");
		}

		self.poll_wake();
	}

	/// # Safety
	/// valid ptr
	#[allow(clippy::missing_panics_doc)]
	unsafe fn process_wake(_: ReqPtr<isize>, arg: Ptr<()>, events: isize) {
		/* Safety: ptr is valid */
		let this = unsafe { arg.cast::<Self>().as_ref() };

		if unlikely(events != PollFlag::In as isize) {
			this.process_wake_cold(events);

			return;
		}

		let mut woken = 0;

		loop {
			const MAX_RESUME: usize = 4;

			#[allow(clippy::unwrap_used)]
			let mut queue = this.wake_queue.lock().unwrap();

			if queue.is_empty() {
				/* finish up reading the event fd with the lock held. resuming woken tasks
				 * with low latency takes priority */
				this.event_fd
					.read()
					.expect_nounwind("Failed to read event fd");
				break;
			}

			let mut requests = [ReqPtr::null(); MAX_RESUME];
			let amount = queue.len().min(MAX_RESUME);

			for (out, request) in requests.iter_mut().zip(queue.drain(0..amount)) {
				*out = request;
			}

			drop(queue);

			trace!(target: this, ">> {} Wakes", amount);

			/* we expect completing the requests to be costly, so we don't hold the lock */
			for request in requests.iter().take(amount) {
				/* Safety: complete the future */
				unsafe { Request::complete(*request, ()) };
			}

			#[allow(clippy::arithmetic_side_effects)]
			(woken += amount);
		}

		#[allow(clippy::arithmetic_side_effects)]
		let wakes = this.expected_wakes.update(|count| count - woken);

		trace!(target: this, "== Woke up {} tasks, {} more expected", woken, wakes);

		if wakes != 0 {
			this.poll_wake();
		}
	}

	pub fn new() -> Result<Self> {
		let thread_pool = ThreadPool::new_with_default_count()?;
		let (features, ring_fd, params) = create_io_uring()?;
		let rings = Rings::new(ring_fd.as_fd(), &params)?;

		/* Safety: params was just initialized by io_uring_setup */
		let queue = unsafe { Queue::new(rings, params) };

		Ok(Self {
			features,
			ring_fd,
			queue,

			to_submit: Cell::new(0),
			to_complete: Cell::new(0),

			expected_wakes: Cell::new(0),
			wake_queue: Mutex::default(),

			event_fd: EventFd::new(CreateFlag::NonBlock.into())?,
			/* Safety: events does not unwind */
			event_request: unsafe { Request::new(Ptr::null(), Self::process_wake) },

			thread_pool
		})
	}

	#[inline(always)]
	fn enter<F>(&self, func: F) -> Result<()>
	where
		F: Fn(&Self, u32) -> OsResult<i32>
	{
		self.queue.submission.sync();

		let mut to_submit = self.to_submit.replace(0);

		if to_submit != 0 {
			trace!(target: self, "<< {} Operations", to_submit);
		}

		#[allow(clippy::arithmetic_side_effects)]
		self.to_complete.update(|count| count + to_submit as u64);

		loop {
			match func(self, to_submit) {
				#[allow(clippy::arithmetic_side_effects, clippy::cast_sign_loss)]
				Ok(submitted) => {
					to_submit -= submitted as u32;

					if likely(to_submit == 0) {
						break Ok(());
					}

					if likely(!self.features.setup_flag_supported(SetupFlag::SubmitAll)) {
						continue;
					}

					break Err(ErrorKind::OutOfMemory.into());
				}

				Err(err) => {
					break match err {
						OsError::Time | OsError::Intr | OsError::Busy if to_submit == 0 => Ok(()),
						OsError::Again => Err(ErrorKind::OutOfMemory.into()),
						_ => Err(err.into())
					};
				}
			}
		}
	}

	fn flush(&self) -> Result<()> {
		let mut flags = BitFlags::<EnterFlag>::default();

		/* we want to flush cqring if possible, but not run any task work */
		if self.queue.needs_flush() {
			flags |= EnterFlag::GetEvents;
		}

		/* Safety: all sqes are valid */
		self.enter(|this, submit| unsafe {
			io_uring_enter(this.ring_fd.as_fd(), submit, 0, flags, None)
		})
	}

	/// Compatibility function for kernels without `ExtArg`
	#[cold]
	#[inline(never)]
	#[allow(clippy::missing_panics_doc)]
	fn submit_and_wait_compat(&self, mut timeout: u64) -> Result<()> {
		let tail = self.queue.completion.read_ring().1;

		self.flush()?;

		/* some requests may have completed from `flush()`, if there are no
		 * completions, our timeout might hang. if we received events, there is no
		 * need to timeout anyway. note that this is a potential race condition.
		 * the application may hang indefinitely when trying to exit if there are no
		 * cqes to be posted after the timeout gets queued
		 */
		if self.queue.completion.read_ring().1 != tail {
			return Ok(());
		}

		/* limit the timeout to one second
		 * on these older kernels to prevent hang
		 */
		timeout = timeout.min(1_000_000_000);

		#[allow(clippy::unwrap_used)]
		let ts = TimeSpec { nanos: timeout.try_into().unwrap(), sec: 0 };

		let op = Op::timeout(ptr!(&ts), 1, 0);

		self.start_async(op, ptr!(&NO_OP));

		/* Safety: all sqes are valid */
		self.enter(|this, submit| unsafe {
			io_uring_enter(
				this.ring_fd.as_fd(),
				submit,
				1,
				EnterFlag::GetEvents.into(),
				None
			)
		})
		.expect_nounwind("Failed to submit timer");

		/* the kernel received our timeout, we can safely release `ts` */
		Ok(())
	}

	fn submit_and_wait(&self, timeout: u64) -> Result<(u32, u32)> {
		let wait = timeout != 0;

		if unlikely(self.to_submit == 0) {
			let ring = self.queue.completion.read_ring();

			if ring.0 != ring.1 {
				/* already have completions */
				return Ok(ring);
			}

			if !self.queue.needs_enter() && !wait {
				/* no pending completions, no submissions, nothing to wait for, nothing to */
				return Ok(ring);
			}
		}

		if likely(self.features.feature_supported(Feature::ExtArg)) {
			/* Safety: all sqes are valid */
			self.enter(|this, submit| unsafe {
				/*
				 * the kernel doesn't read the timespec until it's actually time to wait for
				 * cqes. avoid loss due to branching here and set EXT_ARG on every enter
				 */
				io_uring_enter_timeout(
					this.ring_fd.as_fd(),
					submit,
					wait as u32,
					EnterFlag::GetEvents.into(),
					timeout
				)
			})?;
		} else {
			self.submit_and_wait_compat(timeout)?;
		}

		Ok(self.queue.completion.read_ring())
	}

	#[inline(always)]
	fn run_events(&self, (mut head, mut tail): (u32, u32)) {
		let mask = self.queue.completion.mask;
		let count = tail.wrapping_sub(head);

		if count == 0 {
			return;
		}

		trace!(target: self, ">> {} Completions", count);

		#[allow(clippy::arithmetic_side_effects)]
		self.to_complete.update(|complete| complete - count as u64);

		let complete = |index, update_head: Option<&mut u32>| {
			let CompletionEntry { user_data, result, .. } =
				/* Safety: masked */
				unsafe { self.queue.completion.read(index & mask) };

			if let Some(head) = update_head {
				/*
				 * more requests may be queued in callback, so
				 * update the cqe head here so that we have one more cqe
				 * available for completions before overflow occurs
				 */
				*head = head.wrapping_add(1);
				self.queue.completion.khead.store(*head, Ordering::Release);
			}

			#[allow(clippy::cast_possible_truncation)]
			let request = Ptr::from_addr(user_data as usize);

			/* Safety: complete the future */
			unsafe { Request::complete(request, result as isize) };
		};

		/* prevent this entry from accesed in the loop */
		tail = tail.wrapping_sub(1);

		/* complete the last first as the relevant data may still be in cache
		 * TODO: maybe do this for submission as well?
		 */
		complete(tail, None);

		while head != tail {
			complete(head, Some(&mut head));
		}

		self.queue
			.completion
			.khead
			.store(head.wrapping_add(1), Ordering::Release);
	}

	#[cold]
	#[inline(never)]
	fn push_flush(&self) {
		self.flush()
			.expect_nounwind("Failed to flush submission ring");
	}

	#[inline(always)]
	fn push(&self, request: SubmissionEntry) {
		self.queue.submission.push(request);

		#[allow(clippy::arithmetic_side_effects)]
		self.to_submit.update(|count| count + 1);

		if likely(self.to_submit < self.queue.submission.capacity) {
			return;
		}

		self.push_flush();
	}

	#[inline(always)]
	fn start_async(&self, mut op: SubmissionEntry, request: ReqPtr<isize>) -> Option<isize> {
		op.user_data = request.addr() as u64;

		self.push(op);

		None
	}

	fn poll_wake(&self) {
		/* Safety: args are valid */
		unsafe {
			self.poll(
				self.event_fd.fd().as_raw_fd(),
				PollFlag::In as u32,
				ptr!(&self.event_request)
			)
		};
	}
}

impl Pin for IoUring {
	unsafe fn pin(&mut self) {
		let arg = ptr!(&*self);

		self.event_request.set_arg(arg.cast());
	}
}

/* Safety: functions don't panic */
unsafe impl EngineImpl for IoUring {
	fn has_work(&self) -> bool {
		self.to_complete != 0 || self.to_submit != 0
	}

	#[inline]
	fn work(&self, timeout: u64) -> Result<()> {
		let events = self.submit_and_wait(timeout)?;

		self.run_events(events);

		Ok(())
	}

	fn prepare_wake(&self) -> Result<()> {
		#[allow(clippy::arithmetic_side_effects)]
		if self.expected_wakes.update(|count| count + 1) == 1 {
			self.poll_wake();
		}

		Ok(())
	}

	fn wake(&self, request: ReqPtr<()>) -> Result<()> {
		#[allow(clippy::unwrap_used)]
		let mut queue = self.wake_queue.lock().unwrap();
		let wake = queue.is_empty();

		queue.push_back(request);

		drop(queue);

		if wake {
			self.event_fd.write(1)?;
		}

		Ok(())
	}

	unsafe fn start_work(&self, work: MutPtr<Work<'_>>, request: ReqPtr<bool>) -> CancelWork {
		/* Safety: guaranteed by caller */
		unsafe { self.thread_pool.submit_direct(work, request) }
	}

	unsafe fn cancel_work(&self, cancel: CancelWork) {
		/* Safety: guaranteed by caller */
		unsafe { self.thread_pool.cancel_direct(cancel) }
	}

	unsafe fn cancel(&self, request: ReqPtr<()>) -> Result<()> {
		#[cfg(feature = "tracing")]
		trace!(target: self, "## cancel(request = {:?})", request);

		let mut op = Op::cancel(0);

		op.addr.addr = request.addr() as u64;

		self.start_async(op, ptr!(&NO_OP));

		Ok(())
	}

	unsafe fn open(
		&self, path: Ptr<()>, flags: u32, mode: u32, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::openat(OpenAt::CurrentWorkingDirectory as i32, path, flags, mode, 0);

		self.start_async(op, request)
	}

	fn close_kind(&self) -> OperationKind {
		if unlikely(!self.features.opcode_supported(OpCode::Close)) {
			OperationKind::SyncOffload
		} else {
			OperationKind::Async
		}
	}

	unsafe fn close(&self, fd: RawFd, request: ReqPtr<isize>) -> Option<isize> {
		if unlikely(!self.features.opcode_supported(OpCode::Close)) {
			/* Safety: guaranteed by caller */
			return unsafe { SyncEngine {}.close(fd, request) };
		}

		let op = Op::close(fd);

		self.start_async(op, request)
	}

	unsafe fn read(
		&self, fd: RawFd, buf: MutPtr<()>, len: usize, offset: i64, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::read(fd, buf, len.try_into().unwrap_or(u32::MAX), offset, 0);

		self.start_async(op, request)
	}

	unsafe fn write(
		&self, fd: RawFd, buf: Ptr<()>, len: usize, offset: i64, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::write(fd, buf, len.try_into().unwrap_or(u32::MAX), offset, 0);

		self.start_async(op, request)
	}

	unsafe fn socket(
		&self, domain: u32, socket_type: u32, protocol: u32, request: ReqPtr<isize>
	) -> Option<isize> {
		if unlikely(!self.features.opcode_supported(OpCode::Socket)) {
			/* Safety: guaranteed by caller */
			return unsafe { SyncEngine {}.socket(domain, socket_type, protocol, request) };
		}

		let op = Op::socket(domain, socket_type, protocol, 0, 0);

		self.start_async(op, request)
	}

	unsafe fn accept(
		&self, socket: RawFd, addr: MutPtr<()>, addrlen: MutPtr<i32>, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::accept(socket, addr, addrlen, 0, 0);

		self.start_async(op, request)
	}

	unsafe fn connect(
		&self, socket: RawFd, addr: Ptr<()>, addrlen: i32, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::connect(socket, addr, addrlen);

		self.start_async(op, request)
	}

	unsafe fn recv(
		&self, socket: RawFd, buf: MutPtr<()>, len: usize, flags: u32, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::recv(socket, buf, len.try_into().unwrap_or(u32::MAX), flags);

		self.start_async(op, request)
	}

	unsafe fn recvmsg(
		&self, socket: RawFd, header: MutPtr<MsgHdr>, flags: u32, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::recvmsg(socket, header, flags);

		self.start_async(op, request)
	}

	unsafe fn send(
		&self, socket: RawFd, buf: Ptr<()>, len: usize, flags: u32, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::send(socket, buf, len.try_into().unwrap_or(u32::MAX), flags);

		self.start_async(op, request)
	}

	unsafe fn sendmsg(
		&self, socket: RawFd, header: Ptr<MsgHdr>, flags: u32, request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::sendmsg(socket, header, flags);

		self.start_async(op, request)
	}

	unsafe fn shutdown(&self, socket: RawFd, how: u32, request: ReqPtr<isize>) -> Option<isize> {
		if unlikely(!self.features.opcode_supported(OpCode::Shutdown)) {
			/* Safety: guaranteed by caller */
			return unsafe { SyncEngine {}.shutdown(socket, how, request) };
		}

		let op = Op::shutdown(socket, how);

		self.start_async(op, request)
	}

	unsafe fn bind(
		&self, socket: RawFd, addr: Ptr<()>, addrlen: i32, request: ReqPtr<isize>
	) -> Option<isize> {
		/* Safety: guaranteed by caller */
		unsafe { SyncEngine {}.bind(socket, addr, addrlen, request) }
	}

	unsafe fn listen(&self, socket: RawFd, backlog: i32, request: ReqPtr<isize>) -> Option<isize> {
		/* Safety: guaranteed by caller */
		unsafe { SyncEngine {}.listen(socket, backlog, request) }
	}

	unsafe fn fsync(&self, file: RawFd, request: ReqPtr<isize>) -> Option<isize> {
		let op = Op::fsync(file, 0);

		self.start_async(op, request)
	}

	unsafe fn statx(
		&self, dirfd: RawFd, path: Ptr<()>, flags: u32, mask: u32, statx: MutPtr<Statx>,
		request: ReqPtr<isize>
	) -> Option<isize> {
		let op = Op::statx(dirfd, path, flags, mask, statx);

		self.start_async(op, request)
	}

	unsafe fn poll(&self, fd: RawFd, mask: u32, request: ReqPtr<isize>) -> Option<isize> {
		let op = Op::poll(fd, mask);

		self.start_async(op, request)
	}
}
