use crate::{html, sdk::http::HttpClient, url};
use serde_json::Value;

pub fn client(base_url: &str) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(base_url)
        .with_cookies_for(base_url)
        .with_webview_challenge_fallback()
}

pub fn fetch_document(base_url: &str, target: &str, fixture: &str) -> String {
    client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

pub fn fetch_json(base_url: &str, target: &str, fixture: &str) -> Value {
    let text = client(base_url)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&text).unwrap_or_else(|_| {
        serde_json::from_str(fixture).unwrap_or_else(|_| Value::Array(Vec::new()))
    })
}

pub fn normalize_key(base_url: &str, input: &str) -> String {
    let trimmed = input.trim();
    let without_domain = trimmed
        .strip_prefix(base_url)
        .or_else(|| trimmed.strip_prefix(base_url.trim_end_matches('/')))
        .unwrap_or(trimmed);
    without_domain
        .split('#')
        .next()
        .unwrap_or(without_domain)
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

pub fn key_from_url(base_url: &str, input: &str) -> Option<String> {
    input
        .contains(base_url.trim_start_matches("https://"))
        .then(|| normalize_key(base_url, input))
}

pub fn absolute_url(base_url: &str, input: &str) -> String {
    if input.starts_with("//") {
        format!("https:{input}")
    } else if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else {
        url::join_url(base_url, input)
    }
}

pub fn filter_string(request: &Value, key: &str, default: &str) -> String {
    filter_string_opt(request, key).unwrap_or_else(|| default.to_string())
}

pub fn filter_string_opt(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn filter_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .and_then(|value| {
            value
                .get("value")
                .unwrap_or(value)
                .as_bool()
                .or_else(|| value.as_bool())
        })
        .unwrap_or(default)
}

pub fn preference_bool(request: &Value, key: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(key))
        .and_then(|value| {
            value
                .get("value")
                .unwrap_or(value)
                .as_bool()
                .or_else(|| value.as_bool())
        })
        .unwrap_or(default)
}

pub fn filter_array(request: &Value, key: &str) -> Vec<String> {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .map(|value| value.get("value").unwrap_or(value))
    else {
        return Vec::new();
    };
    match value {
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        Value::String(value) if !value.is_empty() => vec![value.to_string()],
        _ => Vec::new(),
    }
}

pub fn filter_include_exclude(request: &Value, key: &str) -> (Vec<String>, Vec<String>) {
    let Some(value) = request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .or_else(|| request.get(key))
        .map(|value| value.get("value").unwrap_or(value))
    else {
        return (Vec::new(), Vec::new());
    };
    (
        value_array(value.get("include")),
        value_array(value.get("exclude")),
    )
}

pub fn has_active_filters(request: &Value) -> bool {
    request
        .get("filters")
        .and_then(Value::as_object)
        .is_some_and(|filters| {
            filters
                .values()
                .any(|value| match value.get("value").unwrap_or(value) {
                    Value::String(text) => !text.is_empty(),
                    Value::Array(values) => !values.is_empty(),
                    Value::Bool(value) => *value,
                    Value::Object(object) => object.values().any(|nested| match nested {
                        Value::Array(values) => !values.is_empty(),
                        Value::String(text) => !text.is_empty(),
                        Value::Bool(value) => *value,
                        _ => false,
                    }),
                    _ => false,
                })
        })
}

pub fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

pub fn text_after_marker(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

pub fn html_after_marker(body: &str, marker: &str, end: &str) -> Option<String> {
    html::text_between(body, marker, end).filter(|value| !value.trim().is_empty())
}

pub fn script_json(body: &str, script_id: &str) -> Option<Value> {
    let marker = format!("id=\"{script_id}\"");
    let raw = html::text_between(body, &marker, "</script>")
        .or_else(|| html::text_between(body, &format!("id='{script_id}'"), "</script>"))?;
    serde_json::from_str(raw.trim()).ok()
}

pub fn js_array_value(script: &str, variable: &str) -> Option<Value> {
    let start = script.find(variable)?;
    let after = script[start..].find('=').map(|idx| start + idx + 1)?;
    let mut depth = 0i32;
    let mut end = after;
    let mut started = false;
    for (offset, ch) in script[after..].char_indices() {
        match ch {
            '[' => {
                started = true;
                depth += 1;
            }
            ']' if started => {
                depth -= 1;
                if depth == 0 {
                    end = after + offset + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if !started || end <= after {
        return None;
    }
    serde_json::from_str(script[after..end].trim()).ok()
}

pub fn split_values(value: &str) -> Vec<String> {
    value
        .split([',', '\n', ';'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

pub fn has_next_page(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("rel=\"next\"")
        || lower.contains("class=\"next")
        || lower.contains("next page-numbers")
        || lower.contains(">next<")
}

fn value_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}
