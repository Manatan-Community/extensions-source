use crate::{html, sdk::Context};
use serde_json::Value;

pub fn cleanup_text(input: &str) -> String {
    html::strip_tags(input)
}

pub fn normalize_reader_html(input: &str) -> String {
    input
        .replace("<script", "<!-- script")
        .replace("</script>", "script -->")
        .replace("data-src=", "src=")
}

pub fn image_headers(referer: &str) -> Context {
    let mut headers = Context::new();
    headers.insert("Referer".to_string(), referer.to_string());
    headers
}

pub fn decode_fixture_base64(input: &str) -> Vec<u8> {
    input.as_bytes().to_vec()
}

pub fn next_page_url(current_page: u32, has_next: bool) -> Option<String> {
    has_next.then(|| format!("https://novel.example/chapters?page={}", current_page + 1))
}

pub fn request_key(request: &Value, field: &str) -> Option<String> {
    request
        .get(field)
        .and_then(|value| value.get("key").or(Some(value)))
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            request
                .get("key")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
}
