//! The deckwatch server binary. All the code lives in the library
//! (`src/lib.rs`).

/// The fewest worker threads the runtime gets unless `TOKIO_WORKER_THREADS`
/// sets the number (see [`worker_threads`]).
const MIN_WORKER_THREADS: usize = 2;

fn main() {
    let mut runtime = tokio::runtime::Builder::new_multi_thread();
    runtime.enable_all();
    let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    if let Some(workers) = worker_threads(
        std::env::var_os("TOKIO_WORKER_THREADS").is_some(),
        available,
    ) {
        runtime.worker_threads(workers);
    }
    runtime
        .build()
        .expect("failed to start the Tokio runtime")
        .block_on(deckwatch::run());
}

/// How many worker threads to start: the available parallelism, but at
/// least [`MIN_WORKER_THREADS`]. `None` when `TOKIO_WORKER_THREADS` is set,
/// which Tokio then reads itself, as it did under `#[tokio::main]`.
///
/// Tokio's default is `available_parallelism()`, which inside a container
/// follows the CPU limit rounded down, so any limit under 2 CPUs (the chart's
/// default is 200m) gave the server a single worker. Then one task that holds
/// its thread (as plugin calls did before they moved to the blocking pool,
/// #385) stops every other task, the health checks included, and the kubelet
/// restarts the pod. A second worker keeps the server answering. Idle workers
/// cost next to nothing, and the CPU limit still caps what they use.
fn worker_threads(env_set: bool, available: usize) -> Option<usize> {
    (!env_set).then_some(available.max(MIN_WORKER_THREADS))
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
