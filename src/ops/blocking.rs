//! Utilities for running blocking operations

use std::mem::MaybeUninit;

use xx_core::closure::FnCallOnce;
use xx_core::coroutines::block_on_thread_safe;
use xx_core::runtime::{catch_unwind_safe, join};
use xx_core::threadpool::*;

use super::*;

/// Run a blocking operation on a thread pool to prevent blocking the current
/// thread
///
/// # Examples
///
/// ```
/// let large_vec = run_blocking(|_| (0..1_000_000_000).collect::<Vec<_>>()).await;
/// ```
#[asynchronous]
pub async fn run_blocking<F, Output>(func: F) -> Result<Output>
where
	F: FnOnce(&TaskContext) -> Output + Send,
	Output: Send
{
	let driver = internal_get_driver().await;

	check_interrupt().await?;

	let mut output = MaybeUninit::uninit();
	let mut func = FnCallOnce::new(|context: &TaskContext| {
		output.write(catch_unwind_safe(|| func(context)));
	});

	/* Safety: unwinds are caught */
	let mut work = unsafe { Work::new(func.as_dyn()) };

	/* Safety: we are blocked until the work finishes */
	let ran = block_on_thread_safe(unsafe { driver.run_work(ptr!(&mut work)) }).await;

	if ran {
		/* Safety: the value is initialized */
		Ok(join(unsafe { output.assume_init() }))
	} else {
		Err(ErrorKind::Interrupted.into())
	}
}
