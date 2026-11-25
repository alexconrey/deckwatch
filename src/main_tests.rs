use super::*;

#[test]
fn a_cpu_limit_under_two_cores_still_gets_two_workers() {
    // available_parallelism() is 1 for any CPU limit under 2 cores.
    assert_eq!(worker_threads(false, 1), Some(2));
    assert_eq!(worker_threads(false, 2), Some(2));
    assert_eq!(worker_threads(false, 8), Some(8));
}

#[test]
fn tokio_worker_threads_still_decides_when_set() {
    assert_eq!(worker_threads(true, 1), None);
    assert_eq!(worker_threads(true, 8), None);
}
