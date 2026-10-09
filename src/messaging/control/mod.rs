mod client;
mod protocol;
pub(crate) mod server;
pub(crate) use client::{request, request_with_timeout};
pub(crate) use protocol::{Request, Response};
#[cfg(all(test, unix))]
pub(crate) use server::start;

#[cfg(all(test, unix))]
#[path = "tests/socket_test.rs"]
mod socket_tests;
