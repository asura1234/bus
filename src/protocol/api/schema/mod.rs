pub mod agents;
pub mod common;
pub mod copy;
pub mod events;
pub mod layout;
pub mod methods;
pub mod panes;
pub mod responses;
pub mod server;
pub mod session;
pub mod tabs;
pub mod workspaces;

pub use agents::*;
pub use common::*;
pub use events::*;
pub use panes::*;
pub use responses::*;
pub use server::*;
pub use session::*;
pub use tabs::*;
pub use workspaces::*;

fn is_false(value: &bool) -> bool {
    !*value
}

pub use methods::{Method, Request};

#[cfg(test)]
mod tests {
    use super::*;

    #[path = "golden_test.rs"]
    mod golden;
    #[path = "requests_test.rs"]
    mod requests;
    #[path = "responses_test.rs"]
    mod responses;
}
