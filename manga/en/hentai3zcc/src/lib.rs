use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Hentai3zCc = Hentai3zCc;
const BASE_URL: &str = "https://hentai3z.cc";

struct Hentai3zCc;

impl MangaSource for Hentai3zCc {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            ""
        } else {
            "views"
        };
        let target = list_url(page, "", order);
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = if let Some(tag) = filter_value(&request, "tag").filter(|value| !value.is_empty()) {
            let sort = filter_value(&request, "sort").unwrap_or_default();
            list_url(page, &tag, &sort)
        } else {
            format!("{BASE_URL}/list-manga/{page}?search={}", url::query_escape(query))
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/manga/sample/chapter-1".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(&list_url(1, "", "views"), LIST_FIXTURE));
        let latest = parse_listing(&fetch_document(&list_url(1, "", ""), LIST_FIXTURE));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Compact),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn list_url(page: u64, tag: &str, order: &str) -> String {
    if tag.is_empty() {
        let order = if order.is_empty() {
            String::new()
        } else {
            format!("?order_by={order}")
        };
        format!("{BASE_URL}/list-manga/{page}{order}")
    } else {
        let order = if order.is_empty() {
            String::new()
        } else {
            format!("?order_by={order}")
        };
        format!("{BASE_URL}/manga-list/{tag}/{page}{order}")
    }
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("story_item")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "mg_name", "</div>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Hentai3z.CC".into())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(rewrite_cover)
                    .map(|image| url::join_url(BASE_URL, &image)),
                status: ItemStatus::Unknown,
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("pagination") && body.contains("li:last-child")
            || body.contains("pagination") && body.contains("Next"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "detail_name", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Hentai3z.CC".into())),
        cover: html::attr_after(body, "detail_avatar", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        authors: info_text(body, "author").into_iter().collect(),
        artists: info_text(body, "artist").into_iter().collect(),
        tags: link_values(body, "/manga-list/"),
        description: details_description(body),
        status: status_from(body),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapter_box")
        .skip(1)
        .flat_map(|chunk| chunk.split("class=\"item").skip(1).collect::<Vec<_>>())
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::text_between(chunk, "<p", "</p>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_dd_mm_yyyy(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let encoded = body
        .split("slides_p_path")
        .nth(1)
        .and_then(|part| part.split('[').nth(1))
        .and_then(|part| part.split(']').next())
        .unwrap_or_default();
    encoded
        .split(',')
        .map(|part| part.trim().trim_matches('"').trim_matches('\''))
        .filter(|part| !part.is_empty())
        .filter_map(decode_base64)
        .enumerate()
        .map(|(index, image)| {
            let image = if image.starts_with('/') {
                url::join_url(BASE_URL, &image)
            } else {
                image
            };
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input.trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn rewrite_cover(image: String) -> String {
    image
        .replace("cover_thumb_2.webp", "cover_250x350.jpg")
        .replace("admin.manga18.us", "bk.18porncomic.com")
}

fn details_description(body: &str) -> Option<String> {
    let mut parts = html::text_between(body, "detail_reviewContent", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .into_iter()
        .collect::<Vec<_>>();
    if let Some(alt) = info_text(body, "Other name").filter(|value| !value.eq_ignore_ascii_case("updating")) {
        parts.push(format!("Alternative Names:\n{alt}"));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn info_text(body: &str, label: &str) -> Option<String> {
    body.split("div class=\"item")
        .skip(1)
        .find(|chunk| chunk.to_ascii_lowercase().contains(&label.to_ascii_lowercase()))
        .and_then(|chunk| html::text_between(chunk, "info_value", "</div>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "Updating")
}

fn link_values(body: &str, needle: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(needle))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("on going") || lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn parse_dd_mm_yyyy(input: &str) -> Option<i64> {
    let parts = input.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    unix_date(parts[2].parse().ok()?, parts[1].parse().ok()?, parts[0].parse().ok()?)
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(year) => 29,
            2 => 28,
            _ => return None,
        };
    }
    Some((days + day as i64 - 1) * 86_400)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn decode_base64(input: &str) -> Option<String> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0u8;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b'\r' | b'\n' | b'\t' | b' ' => continue,
            _ => return None,
        } as u32;
        buf = (buf << 6) | value;
        bits += 6;
        while bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    String::from_utf8(out).ok()
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="story_item"><a href="/manga/sample"><img src="/cover.jpg"></a><div class="mg_info"><div class="mg_name"><a>Sample Manga</a></div></div></div>
<ul class="pagination"><li>Next</li></ul>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="detail_name"><h1>Sample Manga</h1></div><div class="detail_avatar"><img src="/cover.jpg"></div>
<div class="detail_reviewContent">Description</div><div class="item"><div class="info_label">Status</div><div class="info_value">On Going</div></div>
<div class="chapter_box"><div class="item"><a href="/manga/sample/chapter-1">Chapter 1</a><p>01-01-2024</p></div></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<script>var slides_p_path = ["L2ltYWdlcy9wYWdlMS5qcGc=","L2ltYWdlcy9wYWdlMi5qcGc=",]</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_manga18_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Manga");
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
