use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{
    html, lnreader, novel,
    sdk::{SearchRequest, http::HttpClient},
};
use serde_json::Value;

const SOURCE: Zelluloza = Zelluloza;
const BASE_URL: &str = "https://zelluloza.ru";

struct Zelluloza;

impl NovelSource for Zelluloza {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let listing = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .unwrap_or("popular");
        let sort = if listing == "latest" {
            "0".to_string()
        } else {
            lnreader::filter_string(&request, "sort", "3")
        };
        let genres = lnreader::filter_string(&request, "genres", "0");
        let body = ajax(
            &[
                ("op", "morebooks"),
                ("par1", ""),
                (
                    "par2",
                    &format!("206:0:{genres}:0.{sort}.0.0.0.0.0.0.0.0.0.0.0..0..:{page}"),
                ),
                ("par4", ""),
            ],
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = ajax(
            &[
                ("op", "morebooks"),
                ("par1", query),
                (
                    "par2",
                    &format!("206:0:0:0.0.0.0.0.0.0.10.0.0.0.0.0..0..:{page}"),
                ),
                ("par4", ""),
            ],
            LIST_FIXTURE,
        );
        let entries = parse_listing(&body);
        Ok(Paged {
            has_next_page: !entries.is_empty(),
            entries,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "1".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key = novel::request_key(&request, "novel").unwrap_or_else(|| "1".to_string());
        let body = fetch_document(&format!("{BASE_URL}/books/{key}"), DETAILS_FIXTURE);
        let mut out = Vec::new();
        for chunk in body.split("class=\"w800_m\"").skip(1) {
            if !chunk.contains("chaptfree") {
                continue;
            }
            let href = html::attr_after(chunk, "<a", "href").unwrap_or_default();
            let path = href
                .split('/')
                .filter(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
                .take(2)
                .collect::<Vec<_>>()
                .join("/");
            if path.is_empty() {
                continue;
            }
            out.push(NovelChapter {
                key: path.clone(),
                title: html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value)),
                chapter_number: Some(out.len() as f32 + 1.0),
                url: Some(format!("{BASE_URL}/books/{path}")),
                language: Some("ru".to_string()),
                ..NovelChapter::default()
            });
        }
        Ok(out)
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter").unwrap_or_else(|| "1/1".to_string());
        let parts = key.split('/').collect::<Vec<_>>();
        let book = parts.first().copied().unwrap_or("1");
        let chapter = parts.get(1).copied().unwrap_or("1");
        let body = ajax(
            &[("op", "getbook"), ("par1", book), ("par2", chapter)],
            TEXT_FIXTURE,
        );
        let encrypted = body.split("<END>").next().unwrap_or(&body);
        let mut decoded = encrypted
            .lines()
            .map(decrypt_line)
            .collect::<String>()
            .replace('\r', "")
            .trim()
            .to_string();
        decoded = replace_pair(&decoded, "[*]", "[/]", "b");
        decoded = replace_pair(&decoded, "[_]", "[/]", "u");
        decoded = replace_pair(&decoded, "[-]", "[/]", "s");
        decoded = replace_pair(&decoded, "[~]", "[/]", "i");
        text_response(&key, &decoded)
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(request.clone())?;
        let latest = self.list(with_listing(request, "latest"))?;
        Ok(vec![
            section("popular", "Popular", popular),
            section("latest", "Latest", latest),
        ])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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
        .with_referer(BASE_URL)
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn ajax(form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(format!("{BASE_URL}/ajaxcall/"))
        .referer(format!("{BASE_URL}/search/done/#result"))
        .origin(BASE_URL)
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("display: flex;")
        .skip(1)
        .filter_map(|chunk| {
            let title = html::attr_after(chunk, "class=\"txt\"", "title")?;
            let href = html::attr_after(chunk, "class=\"txt\"", "href").unwrap_or_default();
            let key = href
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .collect::<String>();
            if key.is_empty() {
                return None;
            }
            let cover =
                html::attr_after(chunk, "class=\"shadow\"", "src").map(|src| absolute_url(&src));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
                url: Some(format!("{BASE_URL}/books/{key}")),
                language: Some("ru".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_document(&format!("{BASE_URL}/books/{key}"), DETAILS_FIXTURE);
    CatalogItem {
        key: key.to_string(),
        title: lnreader::text_after_marker(&body, "class=\"bookname\"", "</h2>")
            .unwrap_or_else(|| "Book".to_string()),
        cover: html::attr_after(&body, "class=\"shadow\"", "src").map(|src| absolute_url(&src)),
        description: lnreader::text_after_marker(&body, "id=\"bann_full\"", "</div>")
            .or_else(|| lnreader::text_after_marker(&body, "id=\"bann_short\"", "</div>")),
        authors: lnreader::text_after_marker(&body, "class=\"author_link\"", "</")
            .into_iter()
            .collect(),
        tags: body
            .split("itemprop=\"genre\"")
            .skip(1)
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</span>").map(|value| html::strip_tags(&value))
            })
            .collect(),
        status: if body.contains("Пишется") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Completed
        },
        url: Some(format!("{BASE_URL}/books/{key}")),
        language: Some("ru".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn decrypt_line(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let bytes = input
        .as_bytes()
        .chunks(2)
        .filter_map(|pair| {
            let hi = alphabet(*pair.first()?);
            let lo = alphabet(*pair.get(1)?);
            Some((hi << 4) | lo)
        })
        .collect::<Vec<_>>();
    format!("<p>{}</p>", String::from_utf8_lossy(&bytes))
}

fn replace_pair(input: &str, open: &str, close: &str, tag: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    loop {
        let Some(start) = rest.find(open) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(close) else {
            out.push_str(open);
            out.push_str(after_open);
            break;
        };
        out.push('<');
        out.push_str(tag);
        out.push('>');
        out.push_str(&after_open[..end]);
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
        rest = &after_open[end + close.len()..];
    }
    out
}

fn alphabet(ch: u8) -> u8 {
    match ch as char {
        '~' => 0,
        'H' => 1,
        '^' => 2,
        '@' => 3,
        'f' => 4,
        '0' => 5,
        '5' => 6,
        'n' => 7,
        'r' => 8,
        '=' => 9,
        'W' => 10,
        'L' => 11,
        '7' => 12,
        ' ' => 13,
        'u' => 14,
        'c' => 15,
        _ => 0,
    }
}

fn text_response(key: &str, html_body: &str) -> ExtensionResult<NovelText> {
    let normalized = novel::normalize_reader_html(html_body);
    Ok(NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(format!("{BASE_URL}/books/{key}")),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    })
}

fn section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.to_string(),
        title: title.to_string(),
        style: Some(HomeSectionStyle::Cover),
        entries: page.entries,
        has_more: page.has_next_page,
        ..HomeSection::default()
    }
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .strip_prefix(&format!("{BASE_URL}/books/"))
        .map(|key| {
            key.chars()
                .filter(|ch| ch.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|key| !key.is_empty())
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        format!("{BASE_URL}{input}")
    }
}

fn with_listing(mut request: Value, listing: &str) -> Value {
    if !request.is_object() {
        request = serde_json::json!({});
    }
    if let Some(object) = request.as_object_mut() {
        object.insert("listing".to_string(), Value::String(listing.to_string()));
    }
    request
}

const LIST_FIXTURE: &str = r#"<div style="display: flex;"><a class="txt" title="Sample Book" href="/books/1"></a><img class="shadow" src="/cover.jpg"></div>"#;
const DETAILS_FIXTURE: &str = r#"<h2 class="bookname">Sample Book</h2><img class="shadow" src="/cover.jpg"><div id="bann_full">Sample summary.</div><a class="author_link">Sample Author</a><span itemprop="genre">Fantasy</span><div class="tech_decription">Пишется</div><ul class="g0"><div class="w800_m"><div class="chaptfree"></div><a class="chptitle" href="/books/1/1">Chapter 1</a></div></ul>"#;
const TEXT_FIXTURE: &str = r#"n~n~n~n~n~<END>"#;

export_novel_source!(SOURCE);
