use manatan_extension::export_manga_source;
use manatan_shared::{
    html,
    mangotheme::{MangoThemeConfig, MangoThemeSource},
    sdk::http::HttpClient,
};

const SOURCE: MangoThemeSource<ImperioDaBritannia> = MangoThemeSource::new();
const BASE_URL: &str = "https://imperiodabritannia.net";

struct ImperioDaBritannia;

impl MangoThemeConfig for ImperioDaBritannia {
    const NAME: &'static str = "Sagrado Império da Britannia";
    const BASE_URL: &'static str = BASE_URL;
    const API_URL: &'static str = "https://api.imperiodabritannia.net/api";
    const CDN_URL: &'static str = "https://cdn.imperiodabritannia.net";
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const ENCRYPTION_KEY: &'static str = "mangotoons_encryption_key_2025";
    const WEB_MANGA_PATH: &'static str = "manga";

    fn extra_headers() -> Vec<(&'static str, String)> {
        vec![
            ("X-API-Token", api_token()),
            ("X-Brit-Cache", "true".to_string()),
            ("X-Noencryptionbritta", "1".to_string()),
        ]
    }
}

fn api_token() -> String {
    let Ok(home) = client().get(BASE_URL).browser_document().send_text() else {
        return String::new();
    };
    let Some(env_url) = home
        .split("<link")
        .filter(|chunk| chunk.contains("env"))
        .find_map(|chunk| html::attr(chunk, "href"))
    else {
        return String::new();
    };
    let target = if env_url.starts_with("http") {
        env_url
    } else {
        format!("{BASE_URL}/{}", env_url.trim_start_matches('/'))
    };
    client()
        .get(target)
        .send_text()
        .ok()
        .and_then(|body| token_from_env(&body))
        .unwrap_or_default()
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn token_from_env(body: &str) -> Option<String> {
    body.split("apiToken:")
        .nth(1)
        .and_then(|rest| rest.split('`').nth(1))
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_api_token_from_env_script() {
        assert_eq!(
            token_from_env("window.env={apiToken:`secret-token`,baseUrl:`x`}"),
            Some("secret-token".to_string())
        );
    }
}

export_manga_source!(SOURCE);
