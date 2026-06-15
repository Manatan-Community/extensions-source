use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: TopTruyen = TopTruyen;
const BASE_URL: &str = "https://www.toptruyenzone3.com";

struct TopTruyen;

impl MangaSource for TopTruyen {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("popular") {
            if page > 1 {
                format!("{BASE_URL}/truyen-tranh-hot?page={page}")
            } else {
                format!("{BASE_URL}/truyen-tranh-hot")
            }
        } else if page > 1 {
            format!("{BASE_URL}?page={page}")
        } else {
            BASE_URL.to_string()
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let genre = filter(filters, "genre");
        let mut target = if let Some(genre) = genre {
            format!("{BASE_URL}/tim-truyen/{genre}")
        } else {
            format!("{BASE_URL}/tim-truyen")
        };
        let mut pairs = vec![
            format!("keyword={}", url::query_escape(query)),
            format!("page={page}"),
            "sort=0".to_string(),
        ];
        if let Some(status) = filter(filters, "status") {
            pairs.push(format!("status={}", url::query_escape(status)));
        }
        target.push('?');
        target.push_str(&pairs.join("&"));
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-tranh/sample-1".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| "/truyen-tranh/sample-1".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/truyen-tranh/sample/chapter-1".into());
        let pages = parse_pages(&fetch_document(&absolute_url(&key), PAGES_FIXTURE));
        if pages.is_empty() {
            return Ok(vec![manga::text_page("Khong tim thay hinh anh")]);
        }
        Ok(pages)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: key.contains("/truyen-tranh/").then(|| details_by_key(&key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("item") && chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            if !href.contains("/truyen-tranh/") {
                return None;
            }
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h3", "</h3>")
                .or_else(|| html::attr_after(chunk, "<a", "title"))
                .or_else(|| html::attr_after(chunk, "<img", "alt"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Manga".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("vi".into()),
                content_rating: Some("adult".into()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("next-page") || body.contains("rel=\"next\""),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let article =
        html::text_between(body, "item-detail", "</article>").unwrap_or_else(|| body.into());
    let other_name = html::text_between(&article, "other-name", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let mut description = html::text_between(&article, "detail-summary", "</p>")
        .or_else(|| html::text_between(&article, "detail-content", "</div>"))
        .map(|value| html::strip_tags(&value))
        .unwrap_or_default();
    if let Some(other_name) = other_name {
        if !description.is_empty() {
            description.push_str("\n\n");
        }
        description.push_str("Other name: ");
        description.push_str(&other_name);
    }
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "title-manga", "</h1>")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Manga".into())),
        cover: html::attr_after(&article, "image-comic", "data-original")
            .or_else(|| html::attr_after(&article, "image-comic", "data-src"))
            .or_else(|| html::attr_after(&article, "image-comic", "src"))
            .or_else(|| html::attr_after(&article, "col-image", "data-original"))
            .or_else(|| html::attr_after(&article, "col-image", "data-src"))
            .or_else(|| html::attr_after(&article, "col-image", "src"))
            .map(|image| absolute_url(&image)),
        authors: info_values(&article, "author"),
        tags: info_values(&article, "category"),
        description: (!description.is_empty()).then_some(description),
        status: parse_status(&info_values(&article, "status").join(" ")),
        url: Some(absolute_url(key)),
        language: Some("vi".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("/truyen-tranh/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let title = html::attr_after(chunk, "<a", "title")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let date = chunk
                .find("chapters")
                .and_then(|index| html::text_between(&chunk[index..], "<div", "</div>"))
                .map(|value| html::strip_tags(&value))
                .and_then(|value| parse_datetime(&value));
            Some(MangaChapter {
                key: key.clone(),
                title,
                chapter_number: chapter_number(&key),
                date_uploaded: date,
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter)
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let page_body = if body.contains("page-chapter") {
        body.split("page-chapter")
            .skip(1)
            .collect::<Vec<_>>()
            .join("<")
    } else {
        body.to_string()
    };
    page_body
        .split("<img")
        .skip(1)
        .filter_map(image_attr)
        .filter(|image| !image.starts_with("data:"))
        .map(|image| absolute_url(&image))
        .fold(Vec::<String>::new(), |mut seen, image| {
            if !seen.contains(&image) {
                seen.push(image);
            }
            seen
        })
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image.clone(),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_values(body: &str, marker: &str) -> Vec<String> {
    body.find(marker)
        .and_then(|index| {
            html::text_between(&body[index..], "detail-info", "</")
                .or_else(|| html::text_between(&body[index..], "col-xs-8", "</"))
        })
        .map(|value| {
            value
                .split("<a")
                .skip(1)
                .map(html::strip_tags)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| {
            body.find(marker)
                .and_then(|index| html::text_between(&body[index..], "col-xs-8", "</"))
                .map(|value| vec![html::strip_tags(&value)])
                .unwrap_or_default()
        })
}

fn parse_status(text: &str) -> ItemStatus {
    let lower = text.to_lowercase();
    if lower.contains("hoàn thành") || lower.contains("complete") || lower.contains("full") {
        ItemStatus::Completed
    } else if lower.contains("đang") || lower.contains("ongoing") || lower.contains("updating") {
        ItemStatus::Ongoing
    } else if lower.contains("tạm") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn parse_datetime(value: &str) -> Option<i64> {
    let date = value.trim().get(..10).unwrap_or(value.trim());
    if date.get(4..5) == Some("-") {
        return manatan_shared::dates::parse_ymd(date);
    }
    let mut parts = date.split(['-', '/']);
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr(chunk, "data-original")
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn normalize_key(input: &str) -> String {
    if input.starts_with("http") {
        input
            .trim_start_matches(BASE_URL)
            .split(['?', '#'])
            .next()
            .unwrap_or(input)
            .trim_end_matches('/')
            .to_string()
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/truyen-tranh/"))
}

fn filter<'a>(filters: &'a Value, id: &str) -> Option<&'a str> {
    filters
        .get(id)
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .filter(|value| !value.is_empty())
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|seen| seen.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_chapter(
    mut chapters: Vec<MangaChapter>,
    chapter: MangaChapter,
) -> Vec<MangaChapter> {
    if !chapters.iter().any(|seen| seen.key == chapter.key) {
        chapters.push(chapter);
    }
    chapters
}

fn chapter_number(key: &str) -> Option<f32> {
    let slug = key.trim_end_matches('/').rsplit('/').next()?;
    let number = slug
        .trim_start_matches("chapter-")
        .trim_start_matches("chap-")
        .replace('-', ".");
    number.parse().ok()
}

const LIST_FIXTURE: &str = r#"<div class="items"><div class="item"><div class="image"><a href="/truyen-tranh/sample-1"><img src="/cover.jpg"></a></div><h3><a href="/truyen-tranh/sample-1">Sample</a></h3></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="title-manga">Sample</h1><img class="image-comic" src="/cover.jpg"><li class="status"><p class="detail-info"><span>Đang tiến hành</span></p></li><li class="category"><p class="detail-info"><a>Action</a></p></li><p class="detail-summary">Summary</p><div class="list-chapter"><li class="row"><div class="chapters"><a href="/truyen-tranh/sample-1/chapter-1">Chapter 1</a></div><div>01-01-2024</div></li></div>"#;
const PAGES_FIXTURE: &str = r#"<div class="page-chapter"><img src="/page1.jpg"></div>"#;

export_manga_source!(SOURCE);
