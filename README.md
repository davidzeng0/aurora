# xx-pulse

![](https://github.com/davidyz0/xx-pulse/actions/workflows/build.yml/badge.svg?event=push)

`msrv: 1.80.0 stable`

[Documentation](https://davidyz0.github.io/aurora)

I/O at the speed of light. See [benchmarks](./benchmarks/README.md).

- [Getting started](#getting-started)
- [Thread local safety](#thread-local-safety)
- [Motivation and use cases](./Motivation.md)

### Note:

This library is not ready for production use. Many semantics and APIs are still under development.

This library is currently only available for Linux (kernel 5.6+, other OS's contributions are welcome).<br>
For Windows and Mac users, running in Docker or WSL also work.

The [rust](https://hub.docker.com/_/rust) docker container is sufficient.

### Getting started

The following is a simple echo server example

#### Add dependency
```sh
# support lib and async impls
cargo add --git https://github.com/davidyz0/xx-core.git xx-core

# i/o engine and driver
cargo add --git https://github.com/davidyz0/xx-pulse.git xx-pulse
```

In file `main.rs`
```rust
use xx_pulse::{Tcp, TcpListener};
use xx_pulse::impls::TaskExt;
use xx_core::error::{Error, Result};
use xx_core::macros::duration;

#[xx_pulse::main]
async fn main() -> Result<()> {
    let listener = Tcp::bind("127.0.0.1:8080").await?;

    // Can also timeout operations after 5s
    let listener2: Option<Result<TcpListener>> = Tcp::bind("...")
        .timeout(duration!(5 s))
        .await;

    loop {
        let (mut client, _) = listener.accept().await?;

        xx_pulse::spawn(async move {
            let mut buf = [0; 1024];

            loop {
                let n = client.recv(&mut buf, Default::default()).await?;

                if n == 0 {
                    break;
                }

                let n = client.send(&buf, Default::default()).await?;

                if n == 0 {
                    break;
                }
            }

            Ok::<_, Error>(())
        }).await;
    }
}
```

### Thread local safety

Thread local access is safe because of async/await syntax. <br>
A compiler error prevents usage of `.await` in synchronous functions and closures. <br>
xx-pulse uses cooperative scheduling, so it is impossble to suspend in a closure without using `unsafe`. <br>
To suspend anyway, see [using sync code as if it were async](./Motivation.md#use-sync-code-as-if-it-were-async)

```rust
#[asynchronous]
async fn try_use_thread_local() {
	THREAD_LOCAL.with(|value| {
		// Ok, cannot cause UB
		use_value_synchronously(value);
	});

	THREAD_LOCAL.with(|value| {
		// Compiler error: cannot use .await in a synchronous function/closure!
		do_async_stuff().await;
	});
}
```
