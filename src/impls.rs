use super::*;

/// Extensions for an async task
#[asynchronous(traitext)]
pub trait TaskExt: Task + Sized {
	/// Set a time limit on a task. If the task completes within the duration,
	/// returns `Some` with the result. Otherwise, `None` is returned.
	async fn timeout<Output>(self, duration: Duration) -> Option<Output>
	where
		Self: for<'ctx> Task<Output<'ctx> = Output>
	{
		select_many! {
			res = self => Some(res),
			_ = sleep(duration) => None
		}
		.await
	}
}

impl<T: Task> TaskExt for T {}
