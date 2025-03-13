#![allow(warnings)]

use xx_core::error::*;
use xx_pulse::net::*;
use xx_pulse::*;

#[main]
#[test]
async fn test_udp() -> Result<()> {
	let mut server = Udp::bind("0.0.0.0:0").await?;
	let mut client = Udp::connect(server.local_addr().await?).await?;

	let mut buf = [0u8; 1];

	for i in 0..10 {
		buf[0] = i;
		client.send(&buf, Default::default()).await?;
		buf[0] = 255;
		server.recv(&mut buf, Default::default()).await?;

		assert!(buf[0] == i);
	}

	Ok(())
}

#[main]
#[test]
async fn test_tcp() -> Result<()> {
	let listener = Tcp::bind("0.0.0.0:0").await?;
	let Join((mut server, addr), mut client) = join(
		listener.accept(),
		Tcp::connect(listener.local_addr().await?)
	)
	.await
	.flatten()?;

	let client_addr = client.local_addr().await?;

	assert_eq!(addr, client_addr);

	let mut buf = [0u8; 1];

	for i in 0..10 {
		buf[0] = i;
		client.send(&buf, Default::default()).await?;
		buf[0] = 255;
		server.recv(&mut buf, Default::default()).await?;

		assert!(buf[0] == i);
	}

	Ok(())
}
