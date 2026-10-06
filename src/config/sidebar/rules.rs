use serde::{Deserialize, Serialize};

use super::{SidebarTokenColor, SidebarTokenStyle};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawRule", into = "RawRule")]
pub struct SidebarTokenRule {
    condition: Condition,
    ignore_case: bool,
    style: SidebarTokenStyle,
}

// Deserialization rejects non-finite thresholds, so equality is reflexive.
impl Eq for SidebarTokenRule {}

#[derive(Debug, Clone, PartialEq)]
enum Condition {
    Equals(String),
    Contains(String),
    StartsWith(String),
    GreaterThan(f64),
    LessThan(f64),
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    equals: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    contains: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    starts_with: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ignore_case: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fg: Option<SidebarTokenColor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bold: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dim: Option<bool>,
}

impl TryFrom<RawRule> for SidebarTokenRule {
    type Error = String;

    fn try_from(raw: RawRule) -> Result<Self, Self::Error> {
        let count = [
            raw.equals.is_some(),
            raw.contains.is_some(),
            raw.starts_with.is_some(),
            raw.gt.is_some(),
            raw.lt.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count();
        if count != 1 {
            return Err(
                "sidebar rule requires exactly one of equals, contains, starts_with, gt, lt".into(),
            );
        }
        let condition = if let Some(value) = raw.equals {
            Condition::Equals(value)
        } else if let Some(value) = raw.contains {
            Condition::Contains(value)
        } else if let Some(value) = raw.starts_with {
            Condition::StartsWith(value)
        } else {
            let (value, greater) = match (raw.gt, raw.lt) {
                (Some(value), _) => (value, true),
                (_, Some(value)) => (value, false),
                _ => unreachable!("validated condition count"),
            };
            if !value.is_finite() {
                return Err("sidebar numeric rule threshold must be finite".into());
            }
            if raw.ignore_case.is_some() {
                return Err("ignore_case applies only to sidebar text conditions".into());
            }
            if greater {
                Condition::GreaterThan(value)
            } else {
                Condition::LessThan(value)
            }
        };
        Ok(Self {
            condition,
            ignore_case: raw.ignore_case.unwrap_or(false),
            style: SidebarTokenStyle {
                fg: raw.fg,
                bold: raw.bold,
                dim: raw.dim,
            },
        })
    }
}

impl From<SidebarTokenRule> for RawRule {
    fn from(rule: SidebarTokenRule) -> Self {
        let mut raw = Self {
            ignore_case: rule.ignore_case.then_some(true),
            fg: rule.style.fg,
            bold: rule.style.bold,
            dim: rule.style.dim,
            ..Self::default()
        };
        match rule.condition {
            Condition::Equals(value) => raw.equals = Some(value),
            Condition::Contains(value) => raw.contains = Some(value),
            Condition::StartsWith(value) => raw.starts_with = Some(value),
            Condition::GreaterThan(value) => raw.gt = Some(value),
            Condition::LessThan(value) => raw.lt = Some(value),
        }
        raw
    }
}
