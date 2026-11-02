"""One per-gate budget for direct Rust/Python test workers."""
import os

MAX_TEST_WORKERS = 4
TEST_WORKER_RESOURCE = frozenset({'test-worker-budget'})


def rust_workers() -> int:
    try:
        requested = int(os.environ.get('NEXTEST_TEST_THREADS', str(MAX_TEST_WORKERS)))
    except ValueError as error:
        raise ValueError('NEXTEST_TEST_THREADS must be a positive integer') from error
    if requested <= 0:
        raise ValueError('NEXTEST_TEST_THREADS must be a positive integer')
    return min(requested, MAX_TEST_WORKERS)


def python_workers(files: int) -> int:
    try:
        requested = int(os.environ.get('PYTEST_WORKERS', str(MAX_TEST_WORKERS)))
    except ValueError as error:
        raise ValueError('PYTEST_WORKERS must be a nonnegative integer') from error
    if requested < 0:
        raise ValueError('PYTEST_WORKERS must be a nonnegative integer')
    if files == 1:
        return 0
    return min(requested, MAX_TEST_WORKERS, files) if files else min(requested, MAX_TEST_WORKERS)
