use serde::Deserialize;

pub(crate) const ROOM_AGENT_CONTENT_INTERFACE_V1: &str = "ROOM_AGENT_CONTENT_INTERFACE_V1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContentSelector {
    TestAgentLed,
    Production,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ContentError {
    ProductionUnavailable,
    MalformedBundle,
    UnknownEntry,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct NamedContent {
    pub(crate) name: String,
    pub(crate) sha256: String,
    pub(crate) body: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct ContentBundle {
    pub(crate) interface: String,
    pub(crate) content_version: u32,
    pub(crate) compatibility: u32,
    pub(crate) digest: String,
    pub(crate) system: String,
    pub(crate) agent: String,
    pub(crate) skills: Vec<NamedContent>,
    pub(crate) references: Vec<NamedContent>,
    pub(crate) index: String,
}

impl ContentBundle {
    pub(crate) fn read(&self, kind: &str, name: &str) -> Result<&str, ContentError> {
        match (kind, name) {
            ("system", "system") => Ok(&self.system),
            ("agent", "agent") => Ok(&self.agent),
            ("index", "index") => Ok(&self.index),
            ("skill", name) => self
                .skills
                .iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.body.as_str())
                .ok_or(ContentError::UnknownEntry),
            ("reference", name) => self
                .references
                .iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.body.as_str())
                .ok_or(ContentError::UnknownEntry),
            _ => Err(ContentError::UnknownEntry),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ContentLoader {
    test: ContentBundle,
}

impl ContentLoader {
    pub(crate) fn test_bundle() -> Self {
        let test = serde_json::from_str::<ContentBundle>(include_str!(concat!(
            env!("OUT_DIR"),
            "/bus-test-agent-led-content.json"
        )))
        .expect("build.rs validated and packed test-agent-led content");
        Self { test }
    }

    pub(crate) fn load(&self, selector: ContentSelector) -> Result<ContentBundle, ContentError> {
        match selector {
            ContentSelector::TestAgentLed
                if self.test.interface == ROOM_AGENT_CONTENT_INTERFACE_V1
                    && self.test.compatibility == 1
                    && self.test.content_version > 0 =>
            {
                Ok(self.test.clone())
            }
            ContentSelector::TestAgentLed => Err(ContentError::MalformedBundle),
            ContentSelector::Production => Err(ContentError::ProductionUnavailable),
        }
    }
}
