#![allow(warnings)]

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use xx_core::async_std::sync::RcNotify;
use xx_core::coroutines::take_interrupt;
use xx_pulse::*;

static EXITED: AtomicBool = AtomicBool::new(false);

#[asynchronous]
async fn block2(notify: RcNotify) {
	let result = select(notify.wait(), notify.wait()).await;

	result.flatten().unwrap_err();

	take_interrupt().await;

	let result = join(notify.wait(), notify.wait()).await;

	result.flatten().unwrap_err();
}

#[asynchronous]
async fn block(notify: RcNotify) {
	for _ in 0..100 {
		notify.wait().await.unwrap_err();

		take_interrupt().await;

		spawn(block2(notify.clone())).await;

		println!("run");
	}

	EXITED.store(true, Ordering::Relaxed);

	panic!("test this too");
}

#[test]
fn test_exit() {
	#[main]
	async fn spawn_it() {
		let notify = RcNotify::new();

		spawn(block(notify.clone())).await;
	}

	spawn_it();

	assert!(EXITED.load(Ordering::Relaxed));
}
