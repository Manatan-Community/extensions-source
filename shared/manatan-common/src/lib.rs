use manatan_sdk::{
    client::Client,
    html::{self, ElementRef, Html, Selector},
    Error, Result,
};
use regex::Regex;
use serde_json::Value;
use url::Url;

pub fn get_document(client: &Client, url: &str) -> Result<(Html, String)> {
    let response = client.get(url).send()?.error_for_status()?;
    let final_url = response.final_url().to_owned();
    let document = html::document(response.text()?);
    Ok((document, final_url))
}

pub fn post_form_document(
    client: &Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<(Html, String)> {
    let response = client.post(url).form(form).send()?.error_for_status()?;
    let final_url = response.final_url().to_owned();
    let document = html::document(response.text()?);
    Ok((document, final_url))
}

pub fn selector(value: &str) -> Result<Selector> {
    html::selector(value)
}

pub fn text(element: ElementRef<'_>) -> String {
    html::text(element)
}

pub fn first_text(root: ElementRef<'_>, selector: &Selector) -> Option<String> {
    root.select(selector)
        .next()
        .map(text)
        .filter(|value| !value.is_empty())
}

pub fn first_attr(root: ElementRef<'_>, selector: &Selector, name: &str) -> Option<String> {
    root.select(selector)
        .find_map(|element| element.value().attr(name).map(str::to_owned))
        .filter(|value| !value.is_empty())
}

pub fn attr(element: ElementRef<'_>, name: &str) -> Option<String> {
    element
        .value()
        .attr(name)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn absolute_url(base: &str, candidate: &str) -> Result<String> {
    html::absolute_url(base, candidate)
}

pub fn canonical_url(base: &str, candidate: &str) -> Result<String> {
    let mut url = Url::parse(&absolute_url(base, candidate)?)
        .map_err(|error| Error::new(error.to_string()))?;
    url.set_fragment(None);
    Ok(url.to_string())
}

pub fn path_key(base: &str, candidate: &str) -> Result<String> {
    let url = Url::parse(&absolute_url(base, candidate)?)
        .map_err(|error| Error::new(error.to_string()))?;
    let mut key = url.path().trim_end_matches('/').to_owned();
    if key.is_empty() {
        key.push('/');
    }
    if let Some(query) = url.query() {
        key.push('?');
        key.push_str(query);
    }
    Ok(key)
}

pub fn query_string(filters: &Value, allowed: &[&str]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for key in allowed {
        let Some(value) = filters.get(*key) else {
            continue;
        };
        match value {
            Value::String(value) if !value.is_empty() => {
                serializer.append_pair(key, value);
            }
            Value::Bool(value) => {
                serializer.append_pair(key, if *value { "true" } else { "false" });
            }
            Value::Number(value) => {
                serializer.append_pair(key, &value.to_string());
            }
            Value::Array(values) => {
                for value in values.iter().filter_map(Value::as_str) {
                    serializer.append_pair(key, value);
                }
            }
            _ => {}
        }
    }
    serializer.finish()
}

pub fn extract_number(value: &str) -> Option<f32> {
    let regex =
        Regex::new(r"(?i)(?:chapter|ch\.?|episode|ep\.?|volume|vol\.?)?\s*(-?\d+(?:\.\d+)?)")
            .expect("number regex is valid");
    regex
        .captures(value)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
}

pub fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn require<T>(value: Option<T>, message: impl Into<String>) -> Result<T> {
    value.ok_or_else(|| Error::new(message))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_dns_safe_absolute_urls() {
        assert_eq!(
            absolute_url("https://reader.example/catalog/", "../book/1").unwrap(),
            "https://reader.example/book/1"
        );
    }

    #[test]
    fn serializes_only_allowed_filter_values() {
        let filters = json!({"genre": ["action", "drama"], "page": 2, "ignored": "x"});
        assert_eq!(
            query_string(&filters, &["genre", "page"]),
            "genre=action&genre=drama&page=2"
        );
    }

    #[test]
    fn extracts_decimal_numbers_from_labels() {
        assert_eq!(extract_number("Chapter 12.5: Arrival"), Some(12.5));
        assert_eq!(extract_number("Special"), None);
    }
}
