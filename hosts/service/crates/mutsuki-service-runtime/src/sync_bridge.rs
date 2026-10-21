//! The one bridge for synchronous ServiceHost callers (Runners, lifecycle hooks,
//! control handlers) that must wait on an async Host operation such as a
//! `ConfigService` write.

/// Runs `future` to completion from synchronous code without deadlocking the
/// caller's runtime.
///
/// - Multi-thread Tokio runtime: the worker hands its other tasks off for the
///   duration (`block_in_place`).
/// - Current-thread runtime: the caller *is* the reactor, so blocking on it
///   would deadlock the very future it waits for; the future runs on a
///   dedicated thread with its own current-thread runtime instead.
/// - No runtime: a private current-thread runtime drives it inline.
///
/// # Panics
///
/// Panics if a private runtime cannot be built or its thread panics.
pub fn block_on_host_future<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    fn private_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("config bridge runtime")
    }
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Ok(_) => std::thread::spawn(move || private_runtime().block_on(future))
            .join()
            .expect("config bridge thread"),
        Err(_) => private_runtime().block_on(future),
    }
}
