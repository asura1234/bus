//! Staged evidence contracts for the universal driver, not a state reducer.
//!
//! U01 defines the data and replay oracles before U02–U07 add consumers. No
//! record here authorizes input, binds a request, settles work or schedules retry.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "U01 stages data contracts before production consumers"
    )
)]

pub(crate) mod identity;
pub(crate) mod input;
pub(crate) mod native;

#[cfg(test)]
#[path = "../tests/contracts_test.rs"]
mod tests;
