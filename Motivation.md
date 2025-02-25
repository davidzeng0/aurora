## Why stackful?
Efficiency is a high priority of xx-* libraries.

- [Zero cost async traits](#zero-cost-async-traits) <br>
- [Use sync code as if it were async](#use-sync-code-as-if-it-were-async) <br>
- [Async closures on stable](#async-closures-no-more-poll-functions-or-impl-future-for-myfuture) <br>
- [Allocation free recursion](#allocation-free-recursion) <br>
- [Performance and inlining](#performance-inlining-and-switching) <br>

### Zero cost async traits
```rust
#[asynchronous]
trait MyTrait {
    async fn my_async_func(&mut self);

    // normal functions okay too
    fn my_normal_func(&mut self);
}

#[asynchronous]
async fn dynamic_dispatch(value: &mut dyn MyTrait) {
    // dynamic dispatch! no `Box::new` or any allocations
    value.my_async_func().await;
    value.my_normal_func();
}

#[asynchronous]
async fn monomorphize(value: &mut impl MyTrait) {
    // no dynamic dispatch. inlining is possible
    value.my_async_func().await;
}
```

### Use sync code as if it were async

Every async task has a execution context responsible for scheduling running operations asynchronously. <br>
Normally, it's implicit and hidden from view of normal code, <br>
but we can get a reference to it using [`aurora::get_context`](https://davidzeng0.github.io/kon/kon/coroutines/fn.get_context.html)

We can then use [`aurora::scoped`](https://davidzeng0.github.io/kon/kon/coroutines/fn.scoped.html) to continue execution of an async function later

```rust
use xx_core::coroutines::Context;
use xx_pulse::{get_context, scoped};

struct Adapter<'ctx> {
    async_context: &'ctx Context
}

#[asynchronous]
async fn read_inner(buf: &mut [u8]) {
    ... // do a non-blocking read here, with async capabilities
}

impl std::io::Read for Adapter<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // # Safety
        // Must ensure any lifetimes captured are valid across suspends.
        //
        // This also includes any lifetimes captured by sync code leading up
        // to this function call.
        unsafe {
            scoped(self.async_context, read_inner(buf))
        }

        // For example, the following is invalid
        THREAD_LOCAL.with(|value| {
            // value might be freed after suspend!
            unsafe { scoped(context, do_stuff_with(value)) }

            // without any async. in this case, the constructor
            // of `adapter` must be unsafe!
            std::io::Read::read(adapter);

            // oh no! value might have been freed
            //
            // for the most part, it's safe to use thread locals
            // in this manner. it's only when a fiber gets resumed
            // as a result of a thread local destructor running
            // that causes use-after-free
            do_something_sync_with(value);
        });
    }
}

#[asynchronous]
async fn do_async_read(buf: &mut [u8]) -> std::io::Result<usize> {
    // store the context for use later
    let async_context = get_context();
    let mut reader = Adapter { async_context };

    // do the sync read without blocking the thread
    let read = std::io::Read::read(&mut reader);

    read
}
```

### Async closures (no more poll functions or `impl Future for MyFuture`!)
```rust
#[asynchronous]
async fn call_async_closure(mut func: impl AsyncFnMut(i32)) {
    func.call_mut(5).await;
    func.call_mut(42).await;
}

let mut num = 0;

// note: rustc complains that its unstable, even though
// the proc macro modifies the syntax
//
// use `|| async move`, which does the same thing, to remove
// the error
call_async_closure(async |val| {
    num += val;
}).await;

println!("{num}!"); // 47!
```

### Allocation free recursion
```rust
#[asynchronous]
async fn fibonacci(n: i32) -> i32 {
    if n <= 2 {
        return 1;
    }

    fibonacci(n - 1).await + fibonacci(n - 2).await
}

// Function even gets optimized from O(2^N) to O(N^2)
println!("{}", fibonacci(20).await); // 6765
```

### Performance (inlining and [switching](https://github.com/davidzeng0/xx-core/blob/main/src/coroutines/README.md))

Generally, linear code without state machines optimizes better.