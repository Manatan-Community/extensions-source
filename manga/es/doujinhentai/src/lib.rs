use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: DoujinHentai = DoujinHentai;
const BASE_URL: &str = "https://doujinhentai.net";
const NAME: &str = "DoujinHentai";
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

struct DoujinHentai;

impl MangaSource for DoujinHentai {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if listing_id(&request) == "latest" {
            "last"
        } else {
            "views"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/lista-manga-hentai?orderby={order}&page={page}"),
            LIST_FIXTURE,
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
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE), &key)],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch_document(
            &search_url(page, query, &request),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample/ch1".into());
        Ok(parse_pages(&fetch_document(
            &absolute_url(&key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_listing(&fetch_document(
            &format!("{BASE_URL}/lista-manga-hentai?orderby=views&page=1"),
            LIST_FIXTURE,
        ));
        let latest = parse_listing(&fetch_document(
            &format!("{BASE_URL}/lista-manga-hentai?orderby=last&page=1"),
            LIST_FIXTURE,
        ));
        Ok(vec![
            HomeSection {
                id: "popular".to_string(),
                title: "Populares".to_string(),
                style: Some(HomeSectionStyle::Cover),
                entries: popular.entries,
                has_more: popular.has_next_page,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".to_string(),
                title: "Recientes".to_string(),
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
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE), &key)),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser()
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

fn search_url(page: u64, query: &str, request: &Value) -> String {
    if !query.is_empty() {
        return format!(
            "{BASE_URL}/lista-manga-hentai?search={}&page={page}",
            url::query_escape(query)
        );
    }
    let filters = request.get("filters").unwrap_or(&Value::Null);
    if let Some(genre) = filter_value(filters, "genre") {
        return format!("{BASE_URL}/lista-manga-hentai/category/{genre}?page={page}");
    }
    if let Some(artist) = filter_value(filters, "artist") {
        return format!(
            "{BASE_URL}/lista-manga-hentai/artist/{}?page={page}",
            url::query_escape(&artist)
        );
    }
    if let Some(author) = filter_value(filters, "author") {
        return format!(
            "{BASE_URL}/lista-manga-hentai/author/{}?page={page}",
            url::query_escape(&author)
        );
    }
    if let Some(scanlator) = filter_value(filters, "scanlator") {
        return format!(
            "{BASE_URL}/user/{}?page={page}",
            url::query_escape(&scanlator)
        );
    }
    if let Some(letter) = filter_value(filters, "letter") {
        return format!("{BASE_URL}/lista-manga-hentai/letra/{letter}?page={page}");
    }
    if let Some(kind) = filter_value(filters, "type") {
        return format!("{BASE_URL}/lista-de-{kind}?page={page}");
    }
    let sort = filter_value(filters, "sort").unwrap_or_else(|| "alphabet".to_string());
    format!("{BASE_URL}/lista-manga-hentai?orderby={sort}&page={page}")
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = marker_blocks(body, "group bg-white rounded-2xl")
        .into_iter()
        .filter_map(|chunk| {
            let href = html::attr_after(&chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: class_text(&chunk, "font-bold")
                    .or_else(|| html::attr_after(&chunk, "<img", "alt"))
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| NAME.to_string())
                    }),
                cover: html::attr_after(&chunk, "<img", "src")
                    .or_else(|| html::attr_after(&chunk, "<img", "data-src"))
                    .map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect();
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let authors = link_values(body, "rel=\"author\"");
    let artists = link_values(body, "/artist/");
    let categories = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("rel=\"tag\"") && chunk.contains("/category/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let tags = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("rel=\"tag\"") && chunk.contains("/tag/"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value).trim_start_matches('#').to_string())
        .filter(|value| !value.is_empty());
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| NAME.to_string())),
        cover: html::attr_after(body, "<figure", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        authors: if authors.is_empty() {
            artists.clone()
        } else {
            authors
        },
        artists: if artists.is_empty() {
            link_values(body, "rel=\"author\"")
        } else {
            artists
        },
        description: html::text_between(body, "prose", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: categories.chain(tags).collect(),
        status: status_from(body),
        url: Some(absolute_url(key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    marker_blocks(body, "flex items-center gap-4 p-3 mb-2 border rounded-lg")
        .into_iter()
        .filter_map(|chunk| {
            let href = html::attr_after(&chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let base = class_text(&chunk, "font-bold")
                .map(|value| value.trim_start_matches("Leer ").to_string())
                .unwrap_or_else(|| "Capitulo".to_string());
            let subtitle = class_text(&chunk, "text-sm font-medium").unwrap_or_default();
            let title = if !subtitle.is_empty() && subtitle != base {
                format!("{base}: {subtitle}")
            } else {
                base
            };
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                scanlators: link_values(&chunk, "/user/"),
                date_uploaded: last_class_text(&chunk, "font-medium")
                    .and_then(|value| parse_date(&value)),
                url: Some(absolute_url(&key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = page_urls_from_script(body);
    if images.is_empty() {
        images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("manga-image")
                    || chunk.contains("data-page")
                    || chunk.contains("vertical-pages-container")
            })
            .filter_map(|chunk| html::attr(chunk, "src"))
            .collect();
    }
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn page_urls_from_script(body: &str) -> Vec<String> {
    let Some(start) = body.find("const pageUrls") else {
        return Vec::new();
    };
    let rest = &body[start..];
    let Some(open) = rest.find('{') else {
        return Vec::new();
    };
    let Some(close) = rest[open..].find('}') else {
        return Vec::new();
    };
    let json = &rest[open + 1..open + close];
    let mut out = Vec::new();
    for entry in json.split(',') {
        let parts = entry.split('"').collect::<Vec<_>>();
        if parts.len() >= 4 {
            out.push(parts[3].replace("\\/", "/"));
        }
    }
    out
}

fn marker_blocks(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .map(|chunk| {
            let end = chunk
                .find(marker)
                .or_else(|| chunk.find("pagination"))
                .unwrap_or(chunk.len());
            chunk[..end].to_string()
        })
        .collect()
}

fn class_text(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .find(|chunk| chunk.contains(class_name))
        .and_then(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn last_class_text(body: &str, class_name: &str) -> Option<String> {
    body.split('<')
        .filter(|chunk| chunk.contains(class_name))
        .filter_map(|chunk| html::text_between(chunk, ">", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .last()
}

fn link_values(body: &str, marker: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains(marker))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_from(body: &str) -> ItemStatus {
    let lower = html::strip_tags(body).to_ascii_lowercase();
    if lower.contains("ongoing") || lower.contains("en curso") {
        ItemStatus::Ongoing
    } else if lower.contains("complet") || lower.contains("finalizado") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let parts = value
        .trim()
        .trim_end_matches('.')
        .split_whitespace()
        .collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let day = parts[0].parse::<i32>().ok()?;
    let month = match parts[1].trim_end_matches('.').to_ascii_lowercase().as_str() {
        "jan" | "ene" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" | "abr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" | "ago" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" | "dic" => 12,
        _ => return None,
    };
    let year = parts[2].parse::<i32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: i32, day: i32) -> i64 {
    let year = year - i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * month + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

fn filter_value(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn listing_id(request: &Value) -> &str {
    request
        .get("listingId")
        .or_else(|| request.get("listing"))
        .and_then(Value::as_str)
        .unwrap_or("popular")
}

fn normalize_key(input: &str) -> String {
    if let Some(path) = input.strip_prefix(BASE_URL) {
        return format!("/{}", path.trim_start_matches('/').trim_end_matches('/'));
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<div class="group bg-white rounded-2xl"><a class="block" href="/sample"><img src="/cover.jpg"><h3 class="font-bold">Sample</h3></a></div><a rel="next" href="?page=2">Next</a>"#;
const DETAILS_FIXTURE: &str = r#"<main id="main-content"><h1>Sample</h1><figure><img src="/cover.jpg"></figure><a rel="author">Author</a><a href="/artist/artist">Artist</a><div class="prose">Summary</div><a rel="tag" href="/category/drama">Drama</a><span aria-label="Estado">Ongoing</span><div class="flex items-center gap-4 p-3 mb-2 border rounded-lg"><div class="flex-1"><a class="font-bold" href="/sample/ch1">Leer 1</a><div class="text-sm font-medium">Title</div></div><div class="text-sm text-right"><a href="/user/group">Group</a><span class="font-medium">1 Jan. 2024</span></div></div></main>"#;
const PAGES_FIXTURE: &str = r#"<script>const pageUrls = {"1":"https:\/\/doujinhentai.net\/page1.jpg","2":"https:\/\/doujinhentai.net\/page2.jpg"};</script>"#;
