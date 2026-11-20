use serde_json::Value;

/// What a send does with a recipient that is running a turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SendDelivery {
    /// End the running turn now with the provider's stop key; the message is
    /// the next turn.
    #[default]
    Interrupt,
    /// Join the running turn when the queue head is eligible, else wait.
    Steer,
    /// Wait for the next turn and never join the running one.
    Queue,
}

impl SendDelivery {
    pub(crate) fn parse(value: &Value) -> Result<Self, String> {
        match value.as_str() {
            Some("interrupt") => Ok(Self::Interrupt),
            Some("steer") => Ok(Self::Steer),
            Some("queue") => Ok(Self::Queue),
            Some(other) => Err(format!(
                "--mode must be interrupt, steer or queue, not {other:?}"
            )),
            None => Err("--mode must be interrupt, steer or queue".into()),
        }
    }

    pub(crate) fn queue_only(self) -> bool {
        matches!(self, Self::Interrupt | Self::Queue)
    }
}
