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
    production: Option<ContentBundle>,
}

impl ContentLoader {
    // The runtime calls this for both selectors; it packs every validated embedded bundle.
    pub(crate) fn test_bundle() -> Self {
        let test = serde_json::from_str::<ContentBundle>(include_str!(concat!(
            env!("OUT_DIR"),
            "/bus-test-agent-led-content.json"
        )))
        .expect("build.rs validated and packed test-agent-led content");
        let production = serde_json::from_str::<Option<ContentBundle>>(include_str!(concat!(
            env!("OUT_DIR"),
            "/bus-production-content.json"
        )))
        .expect("build.rs validated and packed production content or its absence");
        Self { test, production }
    }

    #[cfg(test)]
    pub(crate) fn without_production(mut self) -> Self {
        self.production = None;
        self
    }

    pub(crate) fn load(&self, selector: ContentSelector) -> Result<ContentBundle, ContentError> {
        match selector {
            ContentSelector::TestAgentLed => compatible(&self.test).cloned(),
            ContentSelector::Production => match &self.production {
                Some(bundle) => compatible(bundle).cloned(),
                None => Err(ContentError::ProductionUnavailable),
            },
        }
    }
}

fn compatible(bundle: &ContentBundle) -> Result<&ContentBundle, ContentError> {
    if bundle.interface == ROOM_AGENT_CONTENT_INTERFACE_V1
        && bundle.compatibility == 1
        && bundle.content_version > 0
    {
        Ok(bundle)
    } else {
        Err(ContentError::MalformedBundle)
    }
}
