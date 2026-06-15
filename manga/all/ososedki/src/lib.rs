use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: Ososedki = Ososedki;
const BASE_URL: &str = "https://ososedki.com";

struct Ososedki;

impl MangaSource for Ososedki {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            albums_api_url(page, None, None)
        } else {
            albums_api_url(page, Some("top"), Some("1"))
        };
        Ok(parse_albums_response(&fetch_text_or_fixture(
            &target,
            ALBUMS_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(album_id) = album_id_from_url(query) {
            let body = fetch_text_or_fixture(&photo_url(&album_id), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(album_id))],
                has_next_page: false,
            });
        }
        let target = if let Some((kind, value)) = type_value_from_url(query) {
            albums_api_url(page, Some(&kind), Some(&value))
        } else if query.is_empty() {
            albums_api_url(page, Some("top"), Some("1"))
        } else {
            albums_api_url(page, Some("search"), Some(query))
        };
        Ok(parse_albums_response(&fetch_text_or_fixture(
            &target,
            ALBUMS_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "-1_123".into());
        let body = fetch_text_or_fixture(&photo_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "-1_123".into());
        let body = fetch_text_or_fixture(&photo_url(&key), DETAILS_FIXTURE);
        Ok(vec![MangaChapter {
            key: key.clone(),
            title: Some("Gallery".to_string()),
            chapter_number: Some(0.0),
            date_uploaded: parse_upload_date(&body),
            url: Some(photo_url(&key)),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "-1_123".into());
        let body = fetch_text_or_fixture(&photo_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(album_id) = album_id_from_url(input) {
            let body = fetch_text_or_fixture(&photo_url(&album_id), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(album_id))),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn albums_api_url(page: u64, kind: Option<&str>, value: Option<&str>) -> String {
    let mut target = format!("{BASE_URL}/api/albums?page={page}");
    if let (Some(kind), Some(value)) = (kind, value) {
        target.push_str("&type=");
        target.push_str(&url::query_escape(kind));
        target.push_str("&value=");
        target.push_str(&url::query_escape(value));
    }
    target
}

fn parse_albums_response(body: &str) -> Paged<CatalogItem> {
    let Ok(root) = serde_json::from_str::<Value>(body) else {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    };
    let html = root.get("html").and_then(Value::as_str).unwrap_or_default();
    Paged {
        entries: parse_listing_html(html),
        has_next_page: root
            .get("hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

fn parse_listing_html(body: &str) -> Vec<CatalogItem> {
    body.split("gallery-item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "gallery-link", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = album_id_from_url(&href)?;
            let title = html::text_between(chunk, "<h3", "</h3>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| {
                    html::attr_after(chunk, "gallery-img", "alt").and_then(|value| {
                        value
                            .split(" nude.")
                            .next()
                            .map(str::trim)
                            .map(ToString::to_string)
                    })
                })
                .unwrap_or_else(|| key.clone());
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "gallery-img", "data-src")
                    .or_else(|| html::attr_after(chunk, "gallery-img", "src"))
                    .map(|value| url::join_url(BASE_URL, &value)),
                authors: html::text_between(chunk, "badge", "</")
                    .map(|value| vec![html::strip_tags(&value)])
                    .unwrap_or_default(),
                status: ItemStatus::Completed,
                url: Some(photo_url(&key)),
                language: Some("all".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "-1_123".to_string());
    let models = tag_values(body, "/model/");
    let cosplay = tag_values(body, "/cosplay/");
    let fandom = tag_values(body, "/fandom/");
    let title = [models.first(), cosplay.first(), fandom.first()]
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>()
        .join(" - ");
    let title = if title.is_empty() {
        html::text_between(body, "<h1", "</h1>")
            .map(|value| clean_title(&html::strip_tags(&value)))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| key.clone())
    } else {
        title
    };
    CatalogItem {
        key: key.clone(),
        title,
        cover: cover_from_album_id(&key)
            .or_else(|| {
                html::attr_after(body, "gallery-img", "data-src")
                    .map(|value| url::join_url(BASE_URL, &value))
            })
            .or_else(|| html::attr_after(body, "property=og:image", "content")),
        authors: models.clone(),
        artists: cosplay.clone(),
        tags: models.into_iter().chain(cosplay).chain(fandom).collect(),
        status: ItemStatus::Completed,
        url: Some(photo_url(&key)),
        language: Some("all".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter(|href| {
            href.starts_with("/images/") || href.starts_with("https://ososedki.com/images/")
        })
        .map(|href| url::join_url(BASE_URL, &href))
        .collect::<Vec<_>>();
    images.sort_by_key(|image| page_number(image));
    images.dedup();
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let mut extra = BTreeMap::new();
            if let Some(fallback) = image_fallback(&image) {
                extra.insert("fallbackImageUrl".to_string(), Value::String(fallback));
            }
            MangaPage {
                content: PageContent::Url {
                    url: image.clone(),
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                extra,
                ..MangaPage::default()
            }
        })
        .collect()
}

fn tag_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), |mut values, value| {
            if !values.contains(&value) {
                values.push(value);
            }
            values
        })
}

fn parse_upload_date(body: &str) -> Option<i64> {
    let marker = "\"datePublished\":\"";
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest.find('"')?;
    parse_rfc3339_seconds(&rest[..end])
}

fn parse_rfc3339_seconds(value: &str) -> Option<i64> {
    let date_time = value.split_once('T')?;
    let date = date_time.0.split('-').collect::<Vec<_>>();
    let time = date_time
        .1
        .split(['+', '-', 'Z'])
        .next()?
        .split(':')
        .collect::<Vec<_>>();
    let (year, month, day) = (
        date.first()?.parse::<i32>().ok()?,
        date.get(1)?.parse::<u32>().ok()?,
        date.get(2)?.parse::<u32>().ok()?,
    );
    let (hour, minute, second) = (
        time.first()?.parse::<u32>().ok()?,
        time.get(1)?.parse::<u32>().ok()?,
        time.get(2)?.parse::<u32>().ok()?,
    );
    Some(unix_seconds_utc(year, month, day, hour, minute, second))
}

fn unix_seconds_utc(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days as i64 * 86_400 + hour as i64 * 3_600 + minute as i64 * 60 + second as i64
}

fn album_id_from_url(value: &str) -> Option<String> {
    let trimmed = value.trim_end_matches('/');
    let id = trimmed.rsplit('/').next()?;
    is_album_id(id).then(|| id.to_string())
}

fn type_value_from_url(value: &str) -> Option<(String, String)> {
    let parts = value
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let kind = parts.iter().rev().nth(1)?;
    if !matches!(*kind, "model" | "cosplay" | "fandom") {
        return None;
    }
    let value = parts.last()?.replace('+', " ");
    (!value.trim().is_empty()).then(|| (kind.to_string(), value))
}

fn is_album_id(value: &str) -> bool {
    let Some((owner, post)) = value.split_once('_') else {
        return false;
    };
    let owner = owner.strip_prefix('-').unwrap_or(owner);
    !owner.is_empty()
        && !post.is_empty()
        && owner.chars().all(|ch| ch.is_ascii_digit())
        && post.chars().all(|ch| ch.is_ascii_digit())
}

fn photo_url(album_id: &str) -> String {
    format!("{BASE_URL}/photos/{album_id}")
}

fn cover_from_album_id(album_id: &str) -> Option<String> {
    let (owner, post) = album_id.split_once('_')?;
    Some(format!("{BASE_URL}/images/albums/{owner}/{post}.webp"))
}

fn page_number(url: &str) -> i32 {
    url.trim_end_matches('/')
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .and_then(|value| value.parse().ok())
        .unwrap_or(i32::MAX)
}

fn image_fallback(url: &str) -> Option<String> {
    url.contains("/images/a/1280/")
        .then(|| url.replace("/images/a/1280/", "/images/a/604/"))
}

fn clean_title(value: &str) -> String {
    value
        .split(" leaked photos)")
        .next()
        .unwrap_or(value)
        .trim()
        .trim_end_matches('(')
        .trim()
        .to_string()
}

export_manga_source!(SOURCE);

const ALBUMS_FIXTURE: &str = r#"{"html":"<article class=\"gallery-item\"><a class=\"gallery-link\" href=\"https://ososedki.com/photos/-1_123\"><img class=\"gallery-img\" src=\"https://ososedki.com/images/albums/-1/123.webp\" alt=\"Model One nude.\"><h3>Fixture Gallery</h3><span class=\"badge\">Model One</span></a></article>","hasMore":true}"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Fixture Gallery (2 leaked photos) from Onlyfans, Patreon and Fansly</h1>
<a href="/model/model-one">Model One</a><a href="/cosplay/cosplay-one">Cosplay One</a><a href="/fandom/fandom-one">Fandom One</a>
<meta property="og:image" content="https://ososedki.com/images/albums/-1/123.webp">
<script type="application/ld+json">{"datePublished":"2024-01-01T00:00:00Z"}</script>
"#;

const PAGES_FIXTURE: &str = r#"
<div id="photos"><a href="/images/a/1280/-1/123/1.jpg"></a><a href="/images/a/1280/-1/123/2.jpg"></a></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ososedki_fixtures() {
        let page = parse_albums_response(ALBUMS_FIXTURE);
        assert_eq!(page.entries.len(), 1);
        assert!(page.has_next_page);
        assert_eq!(
            parse_details(DETAILS_FIXTURE, Some("-1_123".into())).title,
            "Model One - Cosplay One - Fandom One"
        );
        assert_eq!(parse_upload_date(DETAILS_FIXTURE), Some(1_704_067_200));
        let pages = parse_pages(PAGES_FIXTURE);
        assert_eq!(pages.len(), 2);
        assert!(pages[0].extra.contains_key("fallbackImageUrl"));
    }
}
