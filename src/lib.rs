use std::time::Duration;

use enumflags2::{bitflags, BitFlags};
use xx_core::coroutines::{self, Context, Environment, Executor, Task};
use xx_core::error::*;
use xx_core::future::{self, future, Future, Progress, ReqPtr, Request};

mod driver;
mod engine;
pub mod fs;
pub mod impls;
pub mod interval;
pub mod macros;
pub mod net;
pub mod ops;
mod runtime;

pub use runtime::Runtime;
pub use xx_core::coroutines::{
	acquire_budget, asynchronous, block_on, check_interrupt, check_interrupt_take, current_budget,
	get_context, interrupt_guard, is_interrupted, scoped, take_interrupt
};
#[doc(inline)]
pub use {interval::*, macros::*, ops::*};

use self::driver::*;
use self::engine::*;
use self::runtime::*;
