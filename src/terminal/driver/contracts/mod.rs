//! Staged evidence contracts for the universal driver, not a state reducer.
//!
//! U01 defines the data and replay oracles before U02–U07 add consumers. No
//! record here authorizes input, binds a request, settles work or schedules retry.
//!
//! Root records expect `dead_code` until a production consumer names them.
//! On rustc 1.96, that expectation walks field types and marks them live, so
//! those referenced types cannot carry an expectation of their own.

pub(crate) mod identity;
pub(crate) mod input;
pub(crate) mod native;

#[cfg(test)]
#[path = "../tests/contracts_test.rs"]
mod tests;
