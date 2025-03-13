//! Utilities for branching and spawning async tasks.

use super::*;

#[asynchronous]
#[allow(clippy::multiple_unsafe_ops_per_block)]
pub(crate) async fn spawn_entry<T, Output>(task: T) -> Output
where
	T: for<'ctx> Task<Output<'ctx> = Output>
{
	let workers = internal_get_pulse_env().await.workers;

	/* Safety: the worker is appended to the list */
	let worker = unsafe { PulseWorker::new().await };

	/* Safety: worker is pinned */
	unsafe { ptr!(workers=>append(ptr!(&worker.node))) };

	task.await
}

/// Joins two tasks A and B and waits for both of them to finish, returning both
/// of their results
///
/// If one task panics, an attempt to cancel the other is made, then the panic
/// resumes on the caller
#[asynchronous]
pub async fn join<T1, T2, O1, O2>(task_1: T1, task_2: T2) -> Join<O1, O2>
where
	T1: for<'ctx> Task<Output<'ctx> = O1>,
	T2: for<'ctx> Task<Output<'ctx> = O2>
{
	let runtime = internal_get_pulse_env().await;

	/* Safety: runtimes and executor live until there are no more workers */
	unsafe { coroutines::join(runtime, task_1, task_2).await }
}

/// Races two tasks A and B and waits
/// for one of them to finish and cancelling the other
///
/// Returns [`Select::First`] if the first task completed first
/// or [`Select::Second`] if the second task completed first
///
/// If both tasks are started successfully, the second parameter
/// in `Select` will contain the result from the task that finished second
///
/// If one of the task panics, the panic is resumed on the caller
#[asynchronous]
pub async fn select<T1, T2, O1, O2>(task_1: T1, task_2: T2) -> Select<O1, O2>
where
	T1: for<'ctx> Task<Output<'ctx> = O1>,
	T2: for<'ctx> Task<Output<'ctx> = O2>
{
	let runtime = internal_get_pulse_env().await;

	/* Safety: runtimes and executor live until there are no more workers */
	unsafe { coroutines::select(runtime, task_1, task_2).await }
}

/// Spawn the async task `T`. This task runs separately allowing the current
/// async task to continue.
///
/// Returns a [`JoinHandle`] which may be used to get the result from the task
///
/// # Examples
///
/// ```
/// let handle = spawn(async move {
/// 	println!("hello world");
///
/// 	5
/// })
/// .await;
///
/// assert_eq!(handle.await, 5);
/// ```
#[asynchronous]
pub async fn spawn<T, Output>(task: T) -> JoinHandle<Output>
where
	T: for<'ctx> Task<Output<'ctx> = Output> + 'static
{
	let runtime = internal_get_pulse_env().await;

	/* Safety: task is static */
	unsafe { coroutines::spawn(runtime, spawn_entry(task)) }
}

#[doc(hidden)]
#[cfg(not(doc))]
pub mod internal {
	use super::*;

	#[asynchronous]
	pub async unsafe fn runtime<#[cx] 'current>() -> &'current PulseContext {
		internal_get_pulse_env().await
	}
}

/// Select from multiple async tasks, running the handler
/// for the task that finishes first and cancelling the rest
///
/// ```
/// let item = select_many! {
/// 	item = channel.recv() => {
/// 		println!("{}", item);
///
/// 		Some(item)
/// 	}
///
/// 	expire = sleep(duration!(5 s)) => {
/// 		println!("got nothing");
///
/// 		None
/// 	}
/// }
/// .await;
/// ```
#[macro_export]
macro_rules! select_many {
	{$($tokens:tt)*} => {
		#[allow(clippy::multiple_unsafe_ops_per_block)]
		/* Safety: runtimes and executor live until there are no more workers */
		unsafe {
			::xx_core::coroutines::select! {
				$crate::ops::branch::internal::runtime().await;
				$($tokens)*
			}
		}
	}
}

pub use select_many;

/// Join multiple async tasks, waiting for all of them to complete
///
/// ```
/// let (a, b) = join_many!(load_file("a.txt"), load_file("b.txt")).await;
/// ```
#[macro_export]
macro_rules! join_many {
	($($tokens:tt)*) => {
		#[allow(clippy::multiple_unsafe_ops_per_block)]
		/* Safety: runtimes and executor live until there are no more workers */
		unsafe {
			::xx_core::coroutines::join! {
				$crate::ops::branch::internal::runtime().await;
				$($tokens)*
			}
		}
	}
}

pub use join_many;
