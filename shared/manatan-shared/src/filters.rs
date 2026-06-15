use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub selected: bool,
}

pub fn checkbox(id: &str, label: &str, selected: bool) -> FilterOption {
    FilterOption {
        id: id.to_string(),
        label: label.to_string(),
        selected,
    }
}

pub fn selected_ids(options: &[FilterOption]) -> Vec<String> {
    options
        .iter()
        .filter(|option| option.selected)
        .map(|option| option.id.clone())
        .collect()
}
