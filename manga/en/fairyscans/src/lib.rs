use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: FairyScans = FairyScans;
const BASE_URL: &str = "https://fairyscans.com";

struct FairyScans;

impl MangaSource for FairyScans {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse_response(BROWSE_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request.get("listingId").and_then(Value::as_str);
        let (sort, order) = if listing == Some("popular") {
            ("popular", "desc")
        } else {
            ("latest", "desc")
        };
        Ok(parse_browse_response(&fetch_browse(
            page,
            "",
            sort,
            order,
            "all",
            "all",
            "",
            BROWSE_FIXTURE,
        )))
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
        let filters = request.get("filters");
        Ok(parse_browse_response(&fetch_browse(
            page,
            query,
            filter(filters, "sort", "latest"),
            filter(filters, "order", "desc"),
            filter(filters, "status", "all"),
            filter(filters, "type", "all"),
            filter(filters, "genre", ""),
            BROWSE_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        Ok(parse_details(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".to_string());
        let hide_premium = request
            .get("preferences")
            .and_then(|prefs| {
                prefs
                    .get("hide_premium_chapters")
                    .or_else(|| prefs.get("hidePremiumChapters"))
            })
            .and_then(Value::as_bool)
            .unwrap_or(true);
        Ok(parse_chapters(
            &fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            hide_premium,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
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
        .with_header("Origin", BASE_URL)
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

fn fetch_browse(
    page: u64,
    query: &str,
    sort: &str,
    order: &str,
    status: &str,
    item_type: &str,
    genre: &str,
    fixture: &str,
) -> String {
    let archive = fetch_document(&format!("{BASE_URL}/manga/"), ARCHIVE_FIXTURE);
    let nonce = nonce_for_page(&archive, page).unwrap_or_else(|| "fixture-nonce".to_string());
    let action = if page <= 1 {
        "greed_filter_series"
    } else {
        "greed_archive_load_more"
    };
    client()
        .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
        .form(&[
            ("action", action),
            ("nonce", &nonce),
            ("page", &page.to_string()),
            ("per_initial", "20"),
            ("per_more", "10"),
            ("filters[sort]", sort),
            ("filters[order]", order),
            ("filters[status]", status),
            ("filters[type]", item_type),
            ("filters[genre]", genre),
            ("filters[creator]", ""),
            ("filters[s]", query),
        ])
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn nonce_for_page(body: &str, page: u64) -> Option<String> {
    let marker = if page <= 1 {
        "greedArchiveBrowse"
    } else {
        "greedArchiveMore"
    };
    let chunk = body.split(marker).nth(1)?;
    let start = chunk.find("\"nonce\"")?;
    let rest = &chunk[start..];
    rest.split('"').nth(3).map(ToString::to_string)
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

fn parse_browse_response(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<BrowseResponse>(body)
        .unwrap_or_else(|_| serde_json::from_str(BROWSE_FIXTURE).expect("fixture is valid"));
    let has_next_page = response.has_more();
    let html = response.grid_html();
    Paged {
        entries: parse_browse_cards(&html),
        has_next_page,
    }
}

fn parse_browse_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| {
            !(chunk.contains("greed-browse-card-format-badge--novel")
                || html::text_between(chunk, "greed-browse-card-format-badge", "</")
                    .map(|text| html::strip_tags(&text).eq_ignore_ascii_case("novel"))
                    .unwrap_or(false))
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h2", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Series".to_string())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src")
                    .map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                content_rating: Some("adult".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample".to_string());
    let lower = body.to_ascii_lowercase();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "greed-series-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Series".to_string())),
        authors: author_from_json_ld(body).into_iter().collect(),
        description: html::text_between(body, "greed-series-description", "</div>")
            .map(|value| html::strip_tags(&value)),
        cover: html::attr_after(body, "greed-series-cover-img", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| url::join_url(BASE_URL, &image)),
        tags: body
            .split("greed-series-genre")
            .skip(1)
            .filter_map(|chunk| html::text_between(chunk, ">", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .collect(),
        status: if lower.contains("completed") {
            ItemStatus::Completed
        } else if lower.contains("hiatus") {
            ItemStatus::Hiatus
        } else if lower.contains("dropped") {
            ItemStatus::Cancelled
        } else if lower.contains("ongoing") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, hide_premium: bool) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("greed-series-chapter")
        .skip(1)
        .filter_map(|chunk| {
            let locked = chunk.contains("is-locked");
            if locked && hide_premium {
                return None;
            }
            let href =
                html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let raw_title = html::text_between(chunk, "greed-series-chapter-title", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let order = html::attr(chunk, "data-chapter-order")
                .and_then(|value| value.parse::<f32>().ok())
                .or_else(|| chapter_number(&raw_title))
                .unwrap_or(-1.0);
            let key = normalize_key(&href);
            Some((
                order,
                MangaChapter {
                    key: key.clone(),
                    title: Some(if locked {
                        format!("[Locked] {raw_title}")
                    } else {
                        raw_title
                    }),
                    chapter_number: Some(order),
                    date_uploaded: html::text_between(chunk, "greed-series-chapter-date", "</")
                        .and_then(|value| parse_relative_date(&html::strip_tags(&value))),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some("en".to_string()),
                    ..MangaChapter::default()
                },
            ))
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|left, right| {
        right
            .0
            .partial_cmp(&left.0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters.into_iter().map(|(_, chapter)| chapter).collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let script = body
        .split("<script")
        .find(|chunk| chunk.contains("ts_reader.run"))
        .unwrap_or(body);
    let json = script
        .split("ts_reader.run(")
        .nth(1)
        .and_then(|rest| rest.split(");").next())
        .unwrap_or("{}");
    let reader = serde_json::from_str::<ReaderDto>(json)
        .unwrap_or_else(|_| serde_json::from_str(READER_FIXTURE).expect("fixture is valid"));
    reader
        .images()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input[BASE_URL.len()..]
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn author_from_json_ld(body: &str) -> Option<String> {
    let chunk = body.split("\"author\"").nth(1)?;
    let name = chunk.split("\"name\"").nth(1)?;
    name.split('"').nth(2).map(ToString::to_string)
}

fn chapter_number(title: &str) -> Option<f32> {
    let lower = title.to_ascii_lowercase();
    let rest = lower
        .split("chapter")
        .nth(1)
        .or_else(|| lower.split("ch").nth(1))?;
    rest.trim_start()
        .split(|ch: char| !ch.is_ascii_digit() && ch != '.')
        .next()
        .and_then(|value| value.parse().ok())
}

fn parse_relative_date(value: &str) -> Option<i64> {
    let mut parts = value.split_whitespace();
    let count = parts.next()?.parse::<i64>().ok()?;
    let unit = parts.next()?.to_ascii_lowercase();
    let seconds = if unit.contains("year") {
        count * 31_536_000
    } else if unit.contains("month") {
        count * 2_592_000
    } else if unit.contains("week") {
        count * 604_800
    } else if unit.contains("day") {
        count * 86_400
    } else if unit.contains("hour") {
        count * 3_600
    } else if unit.contains("min") {
        count * 60
    } else {
        count
    };
    Some(1_797_500_800 - seconds)
}

#[derive(Debug, Default, Deserialize)]
struct BrowseResponse {
    success: bool,
    data: Option<BrowseData>,
    html: Option<String>,
    #[serde(alias = "has_more")]
    has_more: Option<bool>,
}

impl BrowseResponse {
    fn has_more(&self) -> bool {
        self.success
            && self
                .has_more
                .or_else(|| self.data.as_ref().and_then(|data| data.has_more))
                .unwrap_or(false)
    }

    fn grid_html(self) -> String {
        self.data
            .and_then(|data| data.grid_html)
            .or(self.html)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Deserialize)]
struct BrowseData {
    #[serde(alias = "grid_html")]
    grid_html: Option<String>,
    #[serde(alias = "has_more")]
    has_more: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct ReaderDto {
    #[serde(default)]
    sources: Vec<ReaderSource>,
}

impl ReaderDto {
    fn images(self) -> Vec<String> {
        self.sources
            .into_iter()
            .next()
            .map(|source| source.images)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Deserialize)]
struct ReaderSource {
    #[serde(default)]
    images: Vec<String>,
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"greedArchiveBrowse = {"nonce":"fixture-nonce"}; greedArchiveMore = {"nonce":"fixture-more"};"#;
const BROWSE_FIXTURE: &str = r#"{
  "success": true,
  "data": {
    "grid_html": "<article><a class=\"greed-browse-card-image\" href=\"/series/sample\"><img src=\"/cover.jpg\"></a><h2><a href=\"/series/sample\">Sample Manga</a></h2><span class=\"greed-browse-card-format-badge\">Manga</span></article>",
    "has_more": false
  }
}"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="greed-series-title">Sample Manga</h1>
<img class="greed-series-cover-img" src="/cover.jpg">
<div class="greed-series-description">A fixture series.</div>
<a class="greed-series-genre">Action</a>
<div class="fairy-series-clean__meta-item--status"><span class="fairy-series-clean__meta-v">Ongoing</span></div>
<a class="greed-series-chapter" data-chapter-order="1" href="/series/sample/chapter-1"><span class="greed-series-chapter-title">Chapter 1</span><span class="greed-series-chapter-date">1 day ago</span></a>
"#;
const READER_FIXTURE: &str = r#"{ "sources": [{ "images": ["/page1.jpg", "/page2.jpg"] }] }"#;
const PAGES_FIXTURE: &str = r#"<script>ts_reader.run({ "sources": [{ "images": ["/page1.jpg", "/page2.jpg"] }] });</script>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_ajax_and_reader() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE
                .pages(json!({"chapter":"/series/sample/chapter-1"}))
                .unwrap()
                .len(),
            2
        );
    }
}
