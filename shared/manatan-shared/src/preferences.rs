use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preference {
    pub id: String,
    pub label: String,
    pub kind: String,
    #[serde(default)]
    pub default_value: Option<String>,
}

pub fn text(id: &str, label: &str, default_value: Option<&str>) -> Preference {
    Preference {
        id: id.to_string(),
        label: label.to_string(),
        kind: "text".to_string(),
        default_value: default_value.map(ToString::to_string),
    }
}

pub fn toggle(id: &str, label: &str, enabled: bool) -> Preference {
    Preference {
        id: id.to_string(),
        label: label.to_string(),
        kind: "toggle".to_string(),
        default_value: Some(enabled.to_string()),
    }
}
