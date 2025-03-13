#![allow(warnings)]

use std::time::{Duration, Instant};

use xx_core::error::Result;
use xx_pulse::*;

#[main]
#[test]
async fn test_timers() -> Result<()> {
	let start = Instant::now();
	let mut timer = Interval::new(Duration::from_secs(1));

	for _ in 0..5 {
		timer.next().await?;
	}

	let elapsed = start.elapsed();

	assert!(elapsed > Duration::from_secs_f64(4.5) && elapsed < Duration::from_secs_f64(5.5));

	Ok(())
}

#[asynchronous]
async fn async_add(a: i32, b: i32) -> i32 {
	sleep(Duration::from_secs(1)).await.unwrap();

	a + b
}

#[main]
#[test]
async fn test_async_add() {
	let result = async_add(7, 4).await;

	assert_eq!(result, 11);

	let join_handle = spawn(async_add(33, 77)).await;
	let result = join_handle.await;

	assert_eq!(result, 110);
}
