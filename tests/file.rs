#![allow(warnings)]

use xx_core::async_std::io::*;
use xx_pulse::fs::File;
use xx_pulse::*;

#[main]
#[test]
async fn test_file() {
	let mut file = File::open("Cargo.toml").await.unwrap();
	let mut str = String::new();

	file.read_to_string(&mut str).await.unwrap();

	let len = file.stream_len().await.unwrap();

	assert_eq!(len, str.len() as u64);
	assert!(str.contains("[package]"));
}
