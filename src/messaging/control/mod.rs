mod caller;
mod client;
pub(crate) use caller::SendCaller;
mod protocol;
pub(crate) mod server;
pub(crate) use client::{request, request_with_timeout, undelivered};
pub(crate) use protocol::{Request, Response, ASYNC_SEND_FOLLOWER};
#[cfg(all(test, unix))]
pub(crate) use server::start;

#[cfg(all(test, unix))]
#[path = "tests/socket_test.rs"]
mod socket_tests;
