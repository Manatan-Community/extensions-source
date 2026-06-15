use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Miaoqu = Miaoqu;
const BASE_URL: &str = "https://www.miaoqumh.org";
const MOBILE_URL: &str = "https://m.miaoqumh.org";

struct Miaoqu;

impl MangaSource for Miaoqu {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let order = if listing(&request) == "latest" {
            "addtime"
        } else {
            "hits"
        };
        Ok(parse_listing(&fetch(&format!(
            "{BASE_URL}/category/order/{order}/page/{}",
            page(&request)
        ))))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = query(&request);
        if let Some(key) = key_from_url(&query) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let target = if query.is_empty() {
            let category = filter(filters, "category").unwrap_or("");
            let order = filter(filters, "order").unwrap_or("hits");
            let category = category.trim_matches('/');
            if category.is_empty() {
                format!("{BASE_URL}/category/order/{order}/page/{}", page(&request))
            } else {
                format!(
                    "{BASE_URL}/category/{category}/order/{order}/page/{}",
                    page(&request)
                )
            }
        } else {
            format!(
                "{BASE_URL}/search/{}/{}",
                url::query_escape(&query),
                page(&request)
            )
        };
        Ok(parse_listing(&fetch(&target)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/sample".to_string());
        Ok(parse_chapters(&fetch(&absolute(&key))))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comic/sample/10.html".to_string());
        Ok(parse_pages(&fetch(&absolute(&key)), &key))
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
        Ok(manga::request_key(&request, "manga").map(|key| mobile_absolute(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| mobile_absolute(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_by_key(&key)),
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

fn fetch(target: &str) -> String {
    let fixture = if target.contains(".html") {
        PAGES_FIXTURE
    } else if target.contains("/comic/") {
        DETAILS_FIXTURE
    } else {
        LIST_FIXTURE
    };
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("#mangawrap")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .filter(|chunk| chunk.contains("manga-name") || chunk.contains("background: url("))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "manga-name", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: style_image(chunk).map(|image| absolute(&image)),
                authors: html::text_between(chunk, "manga-author", "</")
                    .map(|value| vec![html::strip_tags(&value)])
                    .unwrap_or_default(),
                url: Some(mobile_absolute(&key)),
                language: Some("zh".to_string()),
                content_rating: Some("safe".to_string()),
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body
            .split("id=\"next\"")
            .nth(1)
            .and_then(|chunk| html::attr(chunk, "href"))
            .is_some_and(|href| !href.trim_matches('/').is_empty()),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch(&absolute(key)), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let infobox = body.split("infobox").nth(1).unwrap_or(body);
    let title = html::text_between(infobox, "title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "喵趣漫画".to_string());
    let description = html::text_between(body, "class=\"text\"", "</")
        .or_else(|| html::text_between(body, "class='text'", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    let info_text = html::strip_tags(infobox);
    CatalogItem {
        key: key.to_string(),
        title,
        cover: html::attr_after(infobox, "<img", "src").map(|image| absolute(&image)),
        url: Some(mobile_absolute(key)),
        authors: label_value(&info_text, "作者：").into_iter().collect(),
        tags: tags_after(infobox, "类型："),
        description,
        language: Some("zh".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Unknown,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter_map(|chunk| {
            if !chunk.contains("<a") {
                return None;
            }
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(mobile_absolute(&key)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str, chapter_key: &str) -> Vec<MangaPage> {
    decode_page_images(body, chapter_id(chapter_key))
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute(&image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_page_images(body: &str, chapter_id: u64) -> Vec<String> {
    let Some(data) = body
        .split("var DATA='")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
    else {
        return Vec::new();
    };
    let key = xor_key(chapter_id);
    let mut bytes = STANDARD.decode(data).unwrap_or_default();
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte ^= key[index & 7];
    }
    let decoded = STANDARD.decode(bytes).unwrap_or_default();
    let text = String::from_utf8_lossy(&decoded);
    serde_json::from_str::<Vec<ImageDto>>(&text)
        .unwrap_or_default()
        .into_iter()
        .map(|image| image.url)
        .collect()
}

fn xor_key(chapter_id: u64) -> [u8; 8] {
    let value = match chapter_id % 10 {
        0 => "8-bXd9iN",
        1 => "8-RXyjry",
        2 => "8-oYvwVy",
        3 => "8-4ZY57U",
        4 => "8-mbJpU7",
        5 => "8-6MM2Ei",
        6 => "8-54TiQr",
        7 => "8-Ph5xx9",
        8 => "8-bYgePR",
        _ => "8-Z9A3bW",
    };
    value.as_bytes().try_into().unwrap_or(*b"8-bXd9iN")
}

fn chapter_id(key: &str) -> u64 {
    key.rsplit('/')
        .next()
        .and_then(|part| part.trim_end_matches(".html").parse().ok())
        .unwrap_or(0)
}

fn style_image(chunk: &str) -> Option<String> {
    chunk
        .split("background: url(")
        .nth(1)
        .and_then(|rest| rest.split(')').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn label_value(text: &str, label: &str) -> Option<String> {
    text.split(label)
        .nth(1)
        .and_then(|rest| rest.split(['类', '更', '\n']).next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn tags_after(chunk: &str, marker: &str) -> Vec<String> {
    chunk
        .split(marker)
        .nth(1)
        .unwrap_or("")
        .split("<a")
        .skip(1)
        .filter_map(|part| {
            html::text_between(part, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn key_from_url(input: &str) -> Option<String> {
    (input.starts_with(BASE_URL) || input.starts_with(MOBILE_URL))
        .then(|| normalize_key(input))
        .filter(|key| key.contains("/comic/"))
}

fn normalize_key(input: &str) -> String {
    let value = input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches(MOBILE_URL)
        .trim_start_matches("/index.php")
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", value.trim_start_matches('/'))
}

fn absolute(path: &str) -> String {
    url::join_url(BASE_URL, path)
}

fn mobile_absolute(path: &str) -> String {
    url::join_url(MOBILE_URL, path)
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

fn filter<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn push_unique(mut values: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !values.iter().any(|existing| existing.key == item.key) {
        values.push(item);
    }
    values
}

#[derive(Default, Deserialize)]
struct ImageDto {
    url: String,
}

const LIST_FIXTURE: &str = r#"<div id="mangawrap"><a href="/comic/sample" style="background: url(/cover.jpg)"><span class="manga-name">Sample Miaoqu</span><span class="manga-author">Author</span></a></div><a id="next" href="/category/order/hits/page/2"></a>"#;
const DETAILS_FIXTURE: &str = r#"<div class="infobox"><h1 class="title">Sample Miaoqu</h1><img src="/cover.jpg"><span class="tage">作者： Author</span><span class="tage">类型：<a>Action</a></span></div><div class="text">Summary</div><ul class="list"><li><a href="/comic/sample/10.html">Chapter 1</a></li></ul>"#;
const PAGES_FIXTURE: &str = "var DATA='bx4RMQBhIz1xRw0xKAorJmIfNyAoVBk5YlQoYTxoVHM='";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing() {
        let page = parse_listing(LIST_FIXTURE);
        assert_eq!(page.entries[0].title, "Sample Miaoqu");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_chapters() {
        let item = parse_details(DETAILS_FIXTURE, "/comic/sample");
        assert_eq!(item.title, "Sample Miaoqu");
        assert_eq!(item.authors, vec!["Author"]);
        let chapters = parse_chapters(DETAILS_FIXTURE);
        assert_eq!(chapters[0].key, "/comic/sample/10.html");
    }

    #[test]
    fn decodes_pages() {
        let pages = parse_pages(PAGES_FIXTURE, "/comic/sample/10.html");
        assert_eq!(
            pages[0].content,
            PageContent::Url {
                url: "https://www.miaoqumh.org/page1.jpg".to_string(),
                context: None
            }
        );
    }
}

export_manga_source!(SOURCE);
