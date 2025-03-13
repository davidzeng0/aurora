//! Contains the implementation for [`Interval`]

use super::*;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum MissedTickBehavior {
	#[default]
	Burst,
	Delay,
	Skip
}

/// A timer for executing a task periodically.
#[derive(Clone, Copy)]
pub struct Interval {
	expire: u64,
	delay: u64,
	missed_tick_behavior: MissedTickBehavior
}

impl Interval {
	/// # Panics
	/// if the delay cannot fit into a u64
	#[must_use]
	pub fn new(delay: Duration) -> Self {
		#[allow(clippy::unwrap_used)]
		let delay = delay.as_nanos().try_into().unwrap();
		let expire = 0;

		Self {
			expire,
			delay,
			missed_tick_behavior: Default::default()
		}
	}

	pub fn set_missed_tick_behavior(&mut self, behavior: MissedTickBehavior) {
		self.missed_tick_behavior = behavior;
	}

	#[asynchronous]
	pub async fn next(&mut self) -> Result<()> {
		if self.delay == 0 {
			return Ok(());
		}

		let now = nanotime();

		if self.expire == 0 {
			self.expire = now;
		}

		#[allow(clippy::unwrap_used, clippy::arithmetic_side_effects)]
		match self.missed_tick_behavior {
			MissedTickBehavior::Burst => self.expire = self.expire.checked_add(self.delay).unwrap(),
			MissedTickBehavior::Delay => self.expire = now.checked_add(self.delay).unwrap(),
			MissedTickBehavior::Skip => {
				let mut next = self.expire.checked_add(self.delay).unwrap();

				if now > next {
					next = now.checked_add(self.delay).unwrap();

					/* subsequent iteration, align to next delay boundary */
					let align = (now - self.expire) % self.delay;

					next -= align;
				}

				self.expire = next;
			}
		}

		if self.expire > now {
			timeout(self.expire, TimeoutFlag::Abs.into()).await
		} else {
			Ok(())
		}
	}
}
