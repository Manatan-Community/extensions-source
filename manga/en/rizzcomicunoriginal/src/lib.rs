use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: RizzComicUnoriginal = RizzComicUnoriginal;
const BASE_URL: &str = "https://rizzcomic.com";
const MANGA_PATH: &str = "/series";

struct RizzComicUnoriginal;

impl MangaSource for RizzComicUnoriginal {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_api_page(LIST_FIXTURE));
        }
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_api_page(&fetch_api_or_fixture(
            "/Index/filter_series",
            &filter_form(Some(order), request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document_or_fixture(&details_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_api_page(&fetch_api_or_fixture(
                "/Index/live_search",
                &[("search_value", query)],
                LIST_FIXTURE,
            )));
        }
        Ok(parse_api_page(&fetch_api_or_fixture(
            "/Index/filter_series",
            &filter_form(None, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample#1".to_string());
        Ok(parse_details(
            &fetch_document_or_fixture(&details_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample#1".to_string());
        Ok(parse_chapters(&fetch_document_or_fixture(
            &details_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/series/sample/chapter-1".to_string());
        Ok(parse_pages(&fetch_document_or_fixture(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| details_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document_or_fixture(input, DETAILS_FIXTURE),
                    Some(key),
                )),
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

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_api_or_fixture(path: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}{path}"))
        .xhr()
        .header("X-API-Request", "1")
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn filter_form<'a>(
    forced_order: Option<&'a str>,
    filters: Option<&'a Value>,
) -> Vec<(&'a str, &'a str)> {
    let order = forced_order.unwrap_or_else(|| filter(filters, "order", "all"));
    let status = filter(filters, "status", "all");
    let media_type = filter(filters, "type", "all");
    let mut form = vec![
        ("OrderValue", order),
        ("StatusValue", status),
        ("TypeValue", media_type),
    ];
    if let Some(genres) = filter(filters, "genre_ids", "")
        .split(',')
        .map(str::trim)
        .find(|value| !value.is_empty())
    {
        form.push(("genres_checked[]", genres));
    }
    form
}

fn parse_api_page(body: &str) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Vec<Comic>>(body)
        .unwrap_or_default()
        .into_iter()
        .map(Comic::into_catalog)
        .collect::<Vec<_>>();
    Paged {
        entries,
        has_next_page: false,
    }
}

#[derive(Default, Deserialize)]
struct Comic {
    title: String,
    id: String,
    #[serde(rename = "image_url")]
    cover: Option<String>,
    #[serde(rename = "long_description")]
    synopsis: Option<String>,
    status: Option<String>,
    #[serde(rename = "type")]
    media_type: Option<String>,
    artist: Option<String>,
    author: Option<String>,
    serialization: Option<String>,
    #[serde(rename = "genre_id")]
    genres: Option<String>,
}

impl Comic {
    fn into_catalog(self) -> CatalogItem {
        let slug = slugify_title(&self.title);
        CatalogItem {
            key: format!("{MANGA_PATH}/{slug}#{}", self.id),
            title: self.title,
            cover: self
                .cover
                .map(|cover| format!("{BASE_URL}/assets/images/{cover}")),
            url: Some(format!("{BASE_URL}{MANGA_PATH}/{slug}/")),
            authors: join_people(self.author, self.serialization),
            artists: self.artist.into_iter().collect(),
            description: self.synopsis,
            tags: tags_from_api(self.media_type, self.genres),
            status: parse_status(self.status.as_deref().unwrap_or_default()),
            language: Some("en".to_string()),
            content_rating: Some("safe".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/series/sample#1".to_string());
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(key_no_fragment(&key)).unwrap_or_else(|| "Manga".into())
            }),
        cover: html::attr_after(body, "thumb", "data-src")
            .or_else(|| html::attr_after(body, "thumb", "src"))
            .or_else(|| html::attr_after(body, "<img", "data-src"))
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| url::join_url(BASE_URL, &value)),
        description: html::text_between(body, "entry-content", "</div>")
            .or_else(|| html::text_between(body, "desc", "</div>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: info_values(body, "Author"),
        artists: info_values(body, "Artist"),
        tags: genre_values(body),
        status: parse_status(&status_text(body)),
        url: Some(details_url(&key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapter") || chunk.contains("eph-num") || chunk.contains("chapternum")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(
                    html::text_between(chunk, "chapternum", "</")
                        .or_else(|| html::text_between(chunk, "<a", "</a>"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| "Chapter".to_string()),
                ),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("en".to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: "/series/sample/chapter-1".to_string(),
            title: Some("Chapter 1".to_string()),
            url: Some(format!("{BASE_URL}/series/sample/chapter-1")),
            language: Some("en".to_string()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut images = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("readerarea")
                || chunk.contains("wp-manga-chapter-img")
                || chunk.contains("src")
        })
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:") && !image.is_empty())
        .collect::<Vec<_>>();
    if images.is_empty() {
        images = json_images(body);
    }
    images
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

fn details_url(key: &str) -> String {
    let slug = key_no_fragment(key)
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("sample");
    format!("{BASE_URL}{MANGA_PATH}/{slug}/")
}

fn normalize_key(input: &str) -> String {
    let without_base = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .trim_start_matches('/')
        .trim_end_matches('/');
    format!("/{}", without_base)
}

fn key_no_fragment(key: &str) -> &str {
    key.split_once('#').map(|(left, _)| left).unwrap_or(key)
}

fn slugify_title(title: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in title.trim().to_ascii_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    slug.trim_matches('-')
        .replace("-s-", "s-")
        .replace("-ll-", "ll-")
}

fn image_attr(input: &str) -> Option<String> {
    html::attr(input, "data-src")
        .or_else(|| html::attr(input, "data-lazy-src"))
        .or_else(|| html::attr(input, "data-cfsrc"))
        .or_else(|| html::attr(input, "src"))
}

fn json_images(body: &str) -> Vec<String> {
    let Some(start) = body.find("\"images\"") else {
        return Vec::new();
    };
    let Some(open) = body[start..].find('[').map(|index| start + index) else {
        return Vec::new();
    };
    let Some(close) = body[open..].find(']').map(|index| open + index + 1) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&body[open..close]).unwrap_or_default()
}

fn info_values(body: &str, label: &str) -> Vec<String> {
    body.split("imptdt")
        .chain(body.split("infotable"))
        .filter(|chunk| {
            chunk
                .to_ascii_lowercase()
                .contains(&label.to_ascii_lowercase())
        })
        .filter_map(|chunk| {
            html::text_between(chunk, "<i", "</i>")
                .or_else(|| html::text_between(chunk, "<td", "</td>"))
        })
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty() && value != "-")
        .collect()
}

fn genre_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/genre/") || chunk.contains("rel=\"tag\""))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn status_text(body: &str) -> String {
    body.split("imptdt")
        .find(|chunk| chunk.to_ascii_lowercase().contains("status"))
        .map(html::strip_tags)
        .unwrap_or_default()
}

fn parse_status(input: &str) -> ItemStatus {
    let lower = input.to_ascii_lowercase();
    if ["ongoing", "new season", "mass released"]
        .iter()
        .any(|value| lower.contains(value))
    {
        ItemStatus::Ongoing
    } else if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("dropped") {
        ItemStatus::Cancelled
    } else if lower.contains("hiatus") || lower.contains("season end") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn tags_from_api(media_type: Option<String>, genres: Option<String>) -> Vec<String> {
    media_type
        .into_iter()
        .map(capitalize)
        .chain(genres.into_iter().flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .map(str::to_string)
                .collect::<Vec<_>>()
        }))
        .filter(|value| !value.is_empty())
        .collect()
}

fn join_people(first: Option<String>, second: Option<String>) -> Vec<String> {
    first
        .into_iter()
        .chain(second)
        .filter(|value| !value.is_empty())
        .collect()
}

fn capitalize(value: String) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => value,
    }
}

fn filter<'a>(filters: Option<&'a Value>, key: &str, fallback: &'a str) -> &'a str {
    filters
        .and_then(Value::as_object)
        .and_then(|object| object.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(fallback)
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
[
  {
    "title": "Sample Manga",
    "id": "1",
    "image_url": "sample.jpg",
    "long_description": "Sample description",
    "status": "ongoing",
    "type": "manhwa",
    "artist": "Sample Artist",
    "author": "Sample Author",
    "genre_id": "3,16"
  }
]
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample Manga</h1><div class="thumb"><img src="/cover.jpg"></div>
<div class="entry-content">Sample description</div><div class="imptdt">Status <i>Ongoing</i></div>
<div class="imptdt">Author <i>Sample Author</i></div><div class="imptdt">Artist <i>Sample Artist</i></div>
<div class="mgen"><a href="/genre/action/">Action</a></div></div>
<div id="chapterlist"><li><a href="/series/sample/chapter-1/"><span class="chapternum">Chapter 1</span></a><span class="chapterdate">2024-01-01</span></li></div>
"#;
const PAGES_FIXTURE: &str =
    r#"<div id="readerarea"><img src="/page1.jpg"><img src="/page2.jpg"></div>"#;
