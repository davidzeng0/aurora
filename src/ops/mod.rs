//! Fundamental async operations

use xx_core::pointer::*;

use super::*;

pub mod blocking;
pub mod branch;
pub mod io;
pub mod timers;

pub use xx_core::coroutines::{Join, JoinHandle, Select};
#[doc(inline)]
pub use {blocking::*, branch::*, timers::*};

#[asynchronous]
async fn internal_get_pulse_env<#[cx] 'current>() -> &'current PulseContext {
	let env = get_context().await.get_environment::<PulseContext>();

	#[allow(clippy::expect_used)]
	env.expect("Cannot use xx-pulse functions with a different runtime")
}

#[asynchronous]
async fn internal_get_driver<#[cx] 'current>() -> &'current Driver {
	let env = internal_get_pulse_env().await;

	/* Safety: driver outlives context */
	unsafe { env.driver.as_ref() }
}
