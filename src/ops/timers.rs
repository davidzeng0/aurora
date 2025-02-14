//! Timers and sleeping

use xx_core::os::time::{self, ClockId};

use super::*;

/// Flags for [`timeout`]
#[bitflags]
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TimeoutFlag {
	/// The `expire` argument is an absolute timeout. The clock source is
	/// [`nanotime`]
	Abs = 1 << 0
}

/// Suspends the current async task for `expire` nanoseconds.
///
/// The `flags` argument specifies options for this timeout. See [`TimeoutFlag`]
/// for more information.
#[asynchronous]
pub async fn timeout(expire: u64, flags: BitFlags<TimeoutFlag>) -> Result<()> {
	let driver = internal_get_driver().await;

	check_interrupt().await?;
	block_on(driver.timeout(expire, flags)).await
}

/// Suspends the current async task for the specified duration.
///
/// # Panics
/// If the duration in nanoseconds is greater than `u64::MAX` (~585 years).
#[asynchronous]
#[allow(clippy::unwrap_used)]
pub async fn sleep(duration: Duration) -> Result<()> {
	timeout(duration.as_nanos().try_into().unwrap(), BitFlags::default()).await
}

/// Yield execution of the current async task
#[asynchronous]
pub async fn yield_now() {
	let _ = sleep(Duration::ZERO).await;
}

/// The clock source for the runtime. This is the [`ClockId::Monotonic`] clock.
/// Returns a time in nanoseconds.
#[allow(clippy::missing_panics_doc, clippy::expect_used)]
#[must_use]
pub fn nanotime() -> u64 {
	time::nanotime(ClockId::Monotonic).expect("Failed to read the clock")
}
