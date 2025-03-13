#![allow(unreachable_pub)]

use std::cell::Cell;

use xx_core::container::intrusive::linked_list::*;
use xx_core::debug;
use xx_core::fiber::*;
use xx_core::pointer::*;
use xx_core::runtime::join;

use super::*;

pub struct PulseContext {
	pub(crate) context: Context,
	pub(crate) driver: Ptr<Driver>,
	pub(crate) executor: Ptr<Executor>,
	pub(crate) workers: Ptr<LinkedList>
}

impl PulseContext {
	/// # Safety
	/// See `Context::run`
	/// the executor, driver, and worker must live for self
	unsafe fn new(driver: Ptr<Driver>, executor: Ptr<Executor>, workers: Ptr<LinkedList>) -> Self {
		/* Safety: guaranteed by caller */
		let waker = unsafe { ptr!(driver=>waker()) };

		Self {
			/* Safety: guaranteed by caller */
			context: unsafe { Context::new::<Self>(Some(waker)) },
			driver,
			executor,
			workers
		}
	}
}

/* Safety: functions don't panic */
unsafe impl Environment for PulseContext {
	fn context(&self) -> &Context {
		&self.context
	}

	fn context_mut(&mut self) -> &mut Context {
		&mut self.context
	}

	unsafe fn from_context(context: &Context) -> &Self {
		let context = container_of!(ptr!(context), Self=>context);

		/* Safety: guaranteed by caller */
		unsafe { context.as_ref() }
	}

	unsafe fn clone(&self) -> Self {
		/* Safety: guaranteed by caller */
		unsafe { Self::new(self.driver, self.executor, self.workers) }
	}

	fn executor(&self) -> Ptr<Executor> {
		self.executor
	}
}

pub struct PulseWorker {
	pub(crate) context: Ptr<Context>,
	pub(crate) node: Node
}

impl PulseWorker {
	/// # Safety
	/// must append to list before dropping
	#[asynchronous]
	pub async unsafe fn new() -> Self {
		Self {
			context: get_context().await.into(),
			node: Node::new()
		}
	}
}

impl Drop for PulseWorker {
	fn drop(&mut self) {
		/* Safety: guaranteed by caller */
		unsafe { self.node.unlink_unchecked() };
	}
}

/// The runtime for xx-pulse
pub struct Runtime {
	driver: Driver,
	executor: Executor,
	workers: LinkedList,
	pool: Pool
}

impl Runtime {
	pub fn new() -> Result<Pinned<Box<Self>>> {
		let runtime = Self {
			driver: Driver::new()?,
			#[allow(clippy::multiple_unsafe_ops_per_block)]
			/* Safety: pool is valid */
			executor: Executor::new(),
			workers: LinkedList::new(),
			pool: Pool::new()
		};

		Ok(runtime.pin_box())
	}

	/// Block on a task `T`, running it to completion. This is the entry point
	/// of any async operation
	pub fn block_on<T, Output>(&self, task: T) -> Output
	where
		T: for<'ctx> Task<Output<'ctx> = Output>
	{
		/* Safety: the env lives until the task finishes */
		#[allow(clippy::multiple_unsafe_ops_per_block)]
		let task = unsafe {
			coroutines::spawn_task(
				PulseContext::new(
					ptr!(&self.driver),
					ptr!(&self.executor),
					ptr!(&self.workers)
				),
				task
			)
		};

		let running = Cell::new(true);

		let block = |_| self.driver.block_while(|| running.get());
		let resume = || running.set(false);

		/* Safety: we are blocked until the future completes */
		join(unsafe { future::block_on(block, resume, task) })
	}
}

impl Drop for Runtime {
	#[allow(clippy::multiple_unsafe_ops_per_block)]
	fn drop(&mut self) {
		/* note: calling `mem::forget` on the runtime is safe, because the runtime
		 * and driver never get deallocated. when the workers try to use the driver,
		 * it hangs indefinitely
		 */
		loop {
			/* to prevent busy looping, move all our nodes to a new list */
			let list = LinkedList::new();

			pin!(list);

			/* Safety: our new list is pinned, and we clear out all nodes before
			 * returning
			 */
			unsafe { self.workers.move_elements(&list) };

			if list.is_empty() {
				break;
			}

			/* Safety: list is pinned */
			while let Some(node) = unsafe { list.pop_front() } {
				/* Safety: all nodes are wrapped in PulseWorker */
				let worker = unsafe { container_of!(node, PulseWorker=>node) };

				/* Safety: the worker must be valid */
				let context = unsafe { ptr!(worker=>context) };

				/* Safety: the worker may not exit immediately, but it unlinks itself on drop */
				unsafe { self.workers.append(node) };

				/* Safety: signal the task to interrupt */
				let result = unsafe { Context::interrupt(context) };

				if let Err(err) = &result {
					debug!("Cancel failed: {:?}", err);
				}
			}

			/* complete any pending i/o */
			self.driver.exit();
		}
	}
}

impl Pin for Runtime {
	#[allow(clippy::multiple_unsafe_ops_per_block)]
	unsafe fn pin(&mut self) {
		/* Safety: we are being pinned */
		unsafe {
			self.executor.pin();
			self.executor.set_pool(ptr!(&self.pool));
			self.workers.pin();
			self.driver.pin();
		}
	}
}
