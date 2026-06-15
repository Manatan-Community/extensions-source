use crate::{
    html, js,
    manga::{self, image_headers},
    sdk::{
        CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
        UrlResolveResult,
        abi::{ExtensionError, ExtensionResult},
        http,
        source::MangaSource,
    },
    url,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::Value;
use std::marker::PhantomData;

pub trait MMLookConfig {
    const NAME: &'static str;
    const BASE_URL: &'static str;
    const DESKTOP_URL: &'static str;
    const LANG: &'static str = "zh";
    const CONTENT_RATING: &'static str = "safe";
    const USE_LEGACY_MANGA_URL: bool = false;
}

pub struct MMLookSource<C>(PhantomData<C>);

impl<C> MMLookSource<C> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<C: MMLookConfig> MangaSource for MMLookSource<C> {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let rank = if listing(&request) == "latest" {
            "5"
        } else {
            "1"
        };
        Ok(Paged {
            entries: parse_rank::<C>(&fetch::<C>(&format!("{}/rank/{rank}", C::DESKTOP_URL))?),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if is_family_url::<C>(query) {
            let key = normalize_key::<C>(query);
            return Ok(Paged {
                entries: vec![fetch_details::<C>(&key)?],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            let limited = query.chars().take(12).collect::<String>();
            let body = client::<C>()
                .post_form_text(format!("{}/s", C::DESKTOP_URL), &[("k", &limited)])?;
            return Ok(Paged {
                entries: parse_search::<C>(&body),
                has_next_page: false,
            });
        }
        if let Some(rank) = filter(&request, "ranking").filter(|value| !value.is_empty()) {
            return Ok(Paged {
                entries: parse_rank::<C>(&fetch::<C>(&format!("{}/rank/{rank}", C::DESKTOP_URL))?),
                has_next_page: false,
            });
        }
        if let Some(category) = filter(&request, "category").filter(|value| !value.is_empty()) {
            return Ok(Paged {
                entries: parse_rank::<C>(&fetch::<C>(&format!(
                    "{}/sort/{category}",
                    C::DESKTOP_URL
                ))?),
                has_next_page: false,
            });
        }
        self.list(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        fetch_details::<C>(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch::<C>(&details_url::<C>(&key))?;
        let mut chapters = parse_chapters::<C>(&body, &key);
        if body.contains("chaplist-more") {
            let id = key.trim_matches('/').split('/').next().unwrap_or(&key);
            let more = client::<C>()
                .post_form_text(format!("{}/morechapter", C::DESKTOP_URL), &[("id", id)])?;
            let response = serde_json::from_str::<MoreResponse>(&more).map_err(extension_error)?;
            chapters.extend(
                response
                    .data
                    .into_iter()
                    .map(|chapter| chapter.to_chapter::<C>(id)),
            );
        }
        Ok(chapters)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let target = chapter_url::<C>(&key);
        Ok(parse_pages::<C>(&fetch::<C>(&target)?, &target))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| details_url::<C>(&key).replace("https:", "http:")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| chapter_url::<C>(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if is_family_url::<C>(input) {
            let key = normalize_key::<C>(input);
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details::<C>(&key)?),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.to_string(),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client<C: MMLookConfig>() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", C::DESKTOP_URL))
        .with_cookies_for(C::DESKTOP_URL)
        .with_cookies_for(C::BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch<C: MMLookConfig>(target: &str) -> ExtensionResult<String> {
    client::<C>().get(target).browser_document().send_text()
}

fn fetch_details<C: MMLookConfig>(key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details::<C>(
        &fetch::<C>(&details_url::<C>(key))?,
        key,
    ))
}

fn extension_error(error: impl std::fmt::Display) -> ExtensionError {
    ExtensionError {
        message: error.to_string(),
    }
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn filter(request: &Value, key: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn is_family_url<C: MMLookConfig>(input: &str) -> bool {
    input.starts_with(C::BASE_URL) || input.starts_with(C::DESKTOP_URL)
}

fn normalize_key<C: MMLookConfig>(input: &str) -> String {
    let key = input
        .trim_end_matches('/')
        .trim_end_matches(".html")
        .rsplit('/')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    if C::USE_LEGACY_MANGA_URL {
        key.to_string()
    } else {
        key.to_string()
    }
}

fn details_url<C: MMLookConfig>(key: &str) -> String {
    format!("{}/{}/", C::DESKTOP_URL, key.trim_matches('/'))
}

fn chapter_url<C: MMLookConfig>(key: &str) -> String {
    format!("{}/{}.html", C::BASE_URL, key.trim_matches('/')).replace("https:", "http:")
}

fn parse_rank<C: MMLookConfig>(body: &str) -> Vec<CatalogItem> {
    body.split("likedata")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key::<C>(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "le-t", "</")
                    .map(|value| html::strip_tags(&value))
                    .unwrap_or_else(|| C::NAME.to_string()),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(C::DESKTOP_URL, &image)),
                description: html::text_between(chunk, "le-j", "</")
                    .map(|value| html::strip_tags(&value)),
                url: Some(details_url::<C>(&key)),
                language: Some(C::LANG.to_string()),
                content_rating: Some(C::CONTENT_RATING.to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_search<C: MMLookConfig>(body: &str) -> Vec<CatalogItem> {
    body.split("item-data")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key::<C>(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "e-title", "</")
                    .or_else(|| html::text_between(chunk, "title", "</"))
                    .map(|value| html::strip_tags(&value))
                    .unwrap_or_else(|| C::NAME.to_string()),
                cover: html::attr_after(chunk, "<img", "data-src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|image| url::join_url(C::DESKTOP_URL, &image)),
                url: Some(details_url::<C>(&key)),
                language: Some(C::LANG.to_string()),
                content_rating: Some(C::CONTENT_RATING.to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details<C: MMLookConfig>(body: &str, key: &str) -> CatalogItem {
    let info = body.split("comicInfo").nth(1).unwrap_or(body);
    let mut item = CatalogItem {
        key: normalize_key::<C>(key),
        title: html::text_between(info, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .unwrap_or_else(|| C::NAME.to_string()),
        cover: html::attr_after(info, "<img", "data-src")
            .or_else(|| html::attr_after(info, "<img", "src"))
            .map(|image| url::join_url(C::DESKTOP_URL, &image)),
        url: Some(details_url::<C>(key)),
        language: Some(C::LANG.to_string()),
        content_rating: Some(C::CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    for part in info.split("<span").skip(1) {
        let text = html::strip_tags(part);
        let value = text.chars().skip(4).collect::<String>().trim().to_string();
        if text.starts_with("作 者：") {
            item.authors = vec![value.clone()];
            item.artists = vec![value];
        } else if text.starts_with("标 签：") {
            item.tags = value.split_whitespace().map(ToString::to_string).collect();
        } else if text.starts_with("状 态：") {
            item.status = if value == "已完结" {
                ItemStatus::Completed
            } else if value == "连载中" {
                ItemStatus::Ongoing
            } else {
                ItemStatus::Unknown
            };
        }
    }
    item.description =
        html::text_between(info, "content", "</").map(|value| html::strip_tags(&value));
    item
}

fn parse_chapters<C: MMLookConfig>(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    body.split("chapterlistload")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let chapter = href.trim_matches('/').trim_end_matches(".html").to_string();
            let key = if chapter.contains('/') {
                chapter
            } else {
                format!("{}/{}", manga_key.trim_matches('/'), chapter)
            };
            Some(MangaChapter {
                key: key.clone(),
                title: Some(html::strip_tags(chunk)),
                url: Some(chapter_url::<C>(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

pub fn parse_pages<C: MMLookConfig>(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .filter(|image| !image.contains("logo"))
        .map(|image| url::join_url(C::BASE_URL, &image))
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = encrypted_payload(body)
            .and_then(|(payload, id)| decrypt_pages(&payload, id))
            .unwrap_or_default();
    }
    if images.is_empty() {
        for payload in js::extract_dean_edwards_payloads(body) {
            images.extend(extract_image_urls::<C>(&payload));
            if images.is_empty() {
                images.extend(
                    encrypted_payload(&payload)
                        .and_then(|(payload, id)| decrypt_pages(&payload, id))
                        .unwrap_or_default(),
                );
            }
        }
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: Some(image_headers(referer)),
            },
            headers: image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn extract_image_urls<C: MMLookConfig>(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        let mut rest = input;
        while let Some(start) = rest.find(quote) {
            rest = &rest[start + 1..];
            let Some(end) = rest.find(quote) else {
                break;
            };
            let value = &rest[..end];
            if looks_like_image_url(value) {
                out.push(url::join_url(C::BASE_URL, value));
            }
            rest = &rest[end + 1..];
        }
    }
    out
}

fn looks_like_image_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    (lower.starts_with("http") || lower.starts_with("//") || lower.starts_with('/'))
        && (lower.contains(".jpg")
            || lower.contains(".jpeg")
            || lower.contains(".png")
            || lower.contains(".webp"))
}

fn encrypted_payload(body: &str) -> Option<(String, usize)> {
    Some((
        body.split("var __c0rst96=\"")
            .nth(1)?
            .split('"')
            .next()?
            .to_string(),
        html::attr_after(body, "readerContainer", "data-id")?
            .parse()
            .ok()?,
    ))
}

pub fn decrypt_pages(data: &str, index: usize) -> Option<Vec<String>> {
    let keys = [
        "smkhy258", "smkd95fv", "md496952", "cdcsdwq", "vbfsa256", "cawf151c", "cd56cvda",
        "8kihnt9", "dso15tlo", "5ko6plhy",
    ];
    let key = keys.get(index)?.as_bytes();
    let mut bytes = STANDARD.decode(data).ok()?;
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte ^= key[i % key.len()];
    }
    serde_json::from_slice(&STANDARD.decode(bytes).ok()?).ok()
}

#[derive(Deserialize)]
struct MoreResponse {
    data: Vec<MoreChapter>,
}

#[derive(Deserialize)]
struct MoreChapter {
    chapterid: String,
    chaptername: String,
}

impl MoreChapter {
    fn to_chapter<C: MMLookConfig>(self, manga_id: &str) -> MangaChapter {
        let key = format!("{manga_id}/{}", self.chapterid);
        MangaChapter {
            key: key.clone(),
            title: Some(self.chaptername),
            url: Some(chapter_url::<C>(&key)),
            ..MangaChapter::default()
        }
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

#[cfg(test)]
const LIST_FIXTURE: &str = r#"<div class="likedata"><a href="/sample/"><img data-src="/cover.jpg"></a><div class="le-t">Sample MMLook</div><div class="le-j">Summary</div></div>"#;
#[cfg(test)]
mod tests {
    use super::*;

    struct FixtureConfig;

    impl MMLookConfig for FixtureConfig {
        const NAME: &'static str = "Fixture";
        const BASE_URL: &'static str = "https://m.example.test";
        const DESKTOP_URL: &'static str = "https://www.example.test";
    }

    #[test]
    fn parses_direct_pages() {
        let pages = parse_pages::<FixtureConfig>(
            r#"<img data-src="/a.jpg"><img src="//cdn.test/b.webp">"#,
            "https://m.example.test/c/1.html",
        );
        assert_eq!(pages.len(), 2);
    }

    #[test]
    fn parses_rank_cards() {
        let entries = parse_rank::<FixtureConfig>(LIST_FIXTURE);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "sample");
    }
}
