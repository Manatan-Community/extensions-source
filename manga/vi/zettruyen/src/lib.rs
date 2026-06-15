use manatan_extension::{
    CatalogItem, Context, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: ZetTruyen = ZetTruyen;
const BASE_URL: &str = "https://www.zettruyen.top";

struct ZetTruyen;

impl MangaSource for ZetTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let sort = if listing(&request) == "popular" {
            "rating"
        } else {
            "latest"
        };
        let body = fetch_document(&search_url(&request, "", sort))?;
        Ok(parse_listing(&body))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)?],
                has_next_page: false,
            });
        }
        let sort = filter(&request, "sort").unwrap_or("latest");
        let body = fetch_document(&search_url(&request, &query, sort))?;
        Ok(parse_listing(&body))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-tranh/sample".to_string());
        details_by_key(&key)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-tranh/sample".to_string());
        fetch_chapters(&key)
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chuong-1".to_string());
        let target = absolute(&key);
        Ok(parse_pages(&fetch_document(&target)?, &target))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Popular".to_string(),
                style: Some(HomeSectionStyle::Featured),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Latest".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: latest.entries,
                has_more: latest.has_next_page,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            let is_chapter = key.contains("/chuong-") || key.contains("/chapter-");
            return Ok(Some(UrlResolveResult {
                item: (!is_chapter).then(|| details_by_key(&key)).transpose()?,
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

fn fetch_document(target: &str) -> ExtensionResult<String> {
    client().get(target).browser_document().send_text()
}

fn fetch_json(target: &str) -> ExtensionResult<String> {
    client()
        .get(target)
        .xhr()
        .header("Accept", "application/json")
        .send_text()
}

fn listing(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing_id"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

fn query(request: &Value) -> String {
    request
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn filter<'a>(request: &'a Value, key: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(key))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn search_url(request: &Value, query: &str, default_sort: &str) -> String {
    let sort = filter(request, "sort").unwrap_or(default_sort);
    let status = filter(request, "status").unwrap_or("all");
    let kind = filter(request, "type").unwrap_or("all");
    let genres = filter(request, "genres").unwrap_or_default();
    format!(
        "{BASE_URL}/tim-kiem-nang-cao?genres={}&status={}&type={}&sort={}&chapterRange=all&name={}&page={}",
        url::query_escape(genres),
        url::query_escape(status),
        url::query_escape(kind),
        url::query_escape(sort),
        url::query_escape(query),
        page(request)
    )
}

fn absolute(key: &str) -> String {
    url::join_url(BASE_URL, key)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen-tranh/"))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim()
        .trim_start_matches(BASE_URL)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "line-clamp-2", "</span>")
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .or_else(|| html::attr(chunk, "title"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".to_string()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr_after(chunk, "<img", "src").map(|image| absolute(&image)),
                url: Some(absolute(&key)),
                language: Some("vi".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        has_next_page: !entries.is_empty(),
        entries,
    }
}

fn details_by_key(key: &str) -> ExtensionResult<CatalogItem> {
    Ok(parse_details(&fetch_document(&absolute(key))?, key))
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let mut item = CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".to_string())),
        cover: html::attr_after(body, "/thumb/", "src").map(|image| absolute(&image)),
        description: html::text_between(body, "comic-content", "</p>")
            .map(|value| html::strip_tags(&value)),
        tags: genres(body),
        url: Some(absolute(key)),
        language: Some("vi".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    };
    if let Some(author) = info_value(body, "Tác giả") {
        item.authors = vec![author.clone()];
        item.artists = vec![author];
    }
    item.status = status_from(info_value(body, "Trạng thái").as_deref().unwrap_or_default());
    item
}

fn info_value(body: &str, label: &str) -> Option<String> {
    body.split(['<', '>'])
        .collect::<Vec<_>>()
        .windows(5)
        .find_map(|window| {
            let text = html::strip_tags(window[0]);
            if text.trim() == label {
                Some(html::strip_tags(window[4]))
            } else {
                None
            }
        })
        .filter(|value| !value.is_empty())
}

fn genres(body: &str) -> Vec<String> {
    let block = body.split("Thể loại").nth(1).unwrap_or_default();
    block
        .split("<a")
        .skip(1)
        .take_while(|chunk| !chunk.contains("</section>") && !chunk.contains("</header>"))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(value: &str) -> ItemStatus {
    let lower = value.to_lowercase();
    if lower.contains("đang tiến hành") {
        ItemStatus::Ongoing
    } else if lower.contains("hoàn thành") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn fetch_chapters(manga_key: &str) -> ExtensionResult<Vec<MangaChapter>> {
    let slug = slug_from_manga_key(manga_key).unwrap_or_else(|| "sample".to_string());
    let mut chapters = Vec::new();
    let mut page = 1;
    loop {
        let target = format!(
            "{BASE_URL}/api/comics/{}/chapters?page={page}&per_page=100&order=desc",
            url::query_escape(&slug)
        );
        let response: ChapterListResponse = serde_json::from_str(&fetch_json(&target)?)
            .map_err(|error| manatan_extension::abi::ExtensionError {
                message: error.to_string(),
            })?;
        let Some(data) = response.data else {
            break;
        };
        for chapter in data.chapters {
            let chapter_slug = chapter.chapter_slug.replace("chapter-", "chuong-");
            let key = format!("/truyen-tranh/{slug}/{chapter_slug}");
            chapters.push(MangaChapter {
                key: key.clone(),
                title: Some(chapter.chapter_name),
                url: Some(absolute(&key)),
                date_uploaded: chapter
                    .updated_at
                    .as_deref()
                    .and_then(|date| parse_api_date(date.split('.').next().unwrap_or(date))),
                ..MangaChapter::default()
            });
        }
        if data.current_page >= data.last_page {
            break;
        }
        page = data.current_page + 1;
    }
    Ok(chapters)
}

fn slug_from_manga_key(key: &str) -> Option<String> {
    let mut parts = key.trim_matches('/').split('/');
    if parts.next()? == "truyen-tranh" {
        parts.next().map(ToString::to_string)
    } else {
        None
    }
}

fn parse_api_date(value: &str) -> Option<i64> {
    let date = value.split('T').next()?;
    manatan_shared::dates::parse_ymd(date)
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("div.center") || chunk.contains("/uploads/") || chunk.contains("/storage/"))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .filter(|image| !image.starts_with("data:"))
        .map(|image| absolute(&image))
        .fold(Vec::new(), |mut out, image| {
            if !out.contains(&image) {
                out.push(image);
            }
            out
        });
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| {
            let mut headers = Context::new();
            headers.insert("Referer".to_string(), referer.to_string());
            headers.insert(
                "Accept".to_string(),
                "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8".to_string(),
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(headers.clone()),
                },
                headers,
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

#[derive(Deserialize)]
struct ChapterListResponse {
    data: Option<ChapterData>,
}

#[derive(Deserialize)]
struct ChapterData {
    #[serde(default)]
    chapters: Vec<ChapterDto>,
    #[serde(default = "one")]
    last_page: u64,
    #[serde(default = "one")]
    current_page: u64,
}

#[derive(Deserialize)]
struct ChapterDto {
    chapter_name: String,
    chapter_slug: String,
    updated_at: Option<String>,
}

fn one() -> u64 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_cards() {
        let page = parse_listing(
            r#"<div class="grid"><a href="/truyen-tranh/sample"><img src="/thumb/a.jpg"><span class="line-clamp-2">Sample</span></a></div>"#,
        );
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].key, "/truyen-tranh/sample");
    }

    #[test]
    fn parses_chapter_response_shape() {
        let response: ChapterListResponse = serde_json::from_str(
            r#"{"success":true,"data":{"chapters":[{"chapter_name":"Chapter 1","chapter_slug":"chapter-1","updated_at":"2024-01-01T00:00:00.000000Z"}],"last_page":1,"current_page":1}}"#,
        )
        .unwrap();
        assert_eq!(response.data.unwrap().chapters[0].chapter_slug, "chapter-1");
    }
}

export_manga_source!(SOURCE);
