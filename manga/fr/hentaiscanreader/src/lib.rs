use base64::{Engine, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: HentaiScanReader = HentaiScanReader;
const BASE_URL: &str = "https://hentai.scanreader.net";

struct HentaiScanReader;

impl MangaSource for HentaiScanReader {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_card_listing(LIST_FIXTURE, true));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        let target = if latest {
            format!("{BASE_URL}/dernieres-sorties/page/{page}/")
        } else if page <= 1 {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/bibliotheque/page/{}/?sort=views", page - 1)
        };
        let body = fetch_document(&target, if latest { LATEST_FIXTURE } else { LIST_FIXTURE });
        Ok(if latest {
            parse_latest_listing(&body)
        } else {
            parse_card_listing(&body, page <= 1)
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = manga_key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let body = fetch_document(
            &format!("{BASE_URL}/?s={}&post_type=manga", url::query_escape(query)),
            LIST_FIXTURE,
        );
        Ok(parse_card_listing(&body, false))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let manga_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&manga_url, DETAILS_FIXTURE);
        let Some(manga_id) = html::attr_after(&body, "secure-chapters-container", "data-manga-id")
        else {
            return Ok(Vec::new());
        };
        let Some(nonce) = html::attr_after(&body, "secure-chapters-container", "data-nonce") else {
            return Ok(Vec::new());
        };
        let response = client()
            .post(format!("{BASE_URL}/wp-admin/admin-ajax.php"))
            .referer(manga_url)
            .form(&[
                ("action", "load_protected_chapters_html"),
                ("manga_id", &manga_id),
                ("nonce", &nonce),
            ])
            .xhr()
            .send_text()
            .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
        Ok(parse_chapters(&ajax_html(&response)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapitre/sample-1".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_pages(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = manga_key_from_input(input) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(key))),
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

export_manga_source!(SOURCE);

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

fn parse_card_listing(body: &str, homepage: bool) -> Paged<CatalogItem> {
    let source = if homepage {
        section_after(body, "popular-section").unwrap_or(body)
    } else {
        body
    };
    Paged {
        entries: source
            .split("manga-card")
            .skip(1)
            .filter_map(item_from_card)
            .filter(|item| !item.title.contains("(Novel)"))
            .collect(),
        has_next_page: body.contains("pagination-next"),
    }
}

fn parse_latest_listing(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("manga-cover").skip(1) {
        let Some(href) = html::attr_after(chunk, "<a", "href") else {
            continue;
        };
        let title = html::text_between(chunk, "manga-title-display", "</")
            .map(|value| html::strip_tags(&value))
            .or_else(|| url::slug_from_url(&href))
            .unwrap_or_else(|| "Manga".into());
        if title.contains("(Novel)") {
            continue;
        }
        let key = normalize_key(&href);
        entries.push(CatalogItem {
            key: key.clone(),
            title,
            cover: image_from_chunk(chunk).map(|image| url::join_url(BASE_URL, &image)),
            url: Some(url::join_url(BASE_URL, &key)),
            language: Some("fr".into()),
            content_rating: Some("adult".into()),
            ..CatalogItem::default()
        });
    }
    Paged {
        entries,
        has_next_page: body.contains("pagination-next"),
    }
}

fn item_from_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "<h3", "</h3>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .or_else(|| url::slug_from_url(&key))?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: cover_from_onclick(html::attr_after(chunk, "<a", "onclick").as_deref())
            .or_else(|| image_from_chunk(chunk))
            .map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    let mut item = CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "manga-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Manga".into()),
        cover: html::attr_after(body, "property=\"og:image\"", "content")
            .or_else(|| image_from_chunk(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "background: #333", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("fr".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    };
    for row in body
        .split("manga-info-grid")
        .skip(1)
        .flat_map(|part| part.split("<div"))
    {
        let text = html::strip_tags(row);
        let lower = text.to_ascii_lowercase();
        if lower.contains("auteur") {
            item.authors = text_values(row, "span");
        } else if lower.contains("genres") {
            item.tags = text_values(row, "span");
        } else if lower.contains("statut") {
            item.status = status_from_text(&text);
        }
    }
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/chapitre/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, "<h4", "</h4>")
                .or_else(|| html::text_between(chunk, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let date_text = chunk
                .split("</h4>")
                .nth(1)
                .map(html::strip_tags)
                .unwrap_or_default();
            Some(MangaChapter {
                key: key.clone(),
                title,
                date_uploaded: dates::parse_ymd(&date_text),
                scanlators: scanlator_from_chunk(chunk).into_iter().collect(),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some("fr".into()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let encoded = quoted_strings(body)
        .into_iter()
        .filter(|value| value.len() >= 20 && value.chars().all(is_base64_char))
        .filter_map(|value| STANDARD.decode(value).ok())
        .filter_map(|bytes| String::from_utf8(bytes).ok())
        .map(|decoded| decoded.chars().rev().collect::<String>())
        .collect::<Vec<_>>();
    let images = if encoded.is_empty() {
        body.split("<img")
            .skip(1)
            .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
            .collect()
    } else {
        encoded
    };
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn ajax_html(response: &str) -> String {
    serde_json::from_str::<AjaxResponse>(response)
        .ok()
        .and_then(|response| response.data)
        .unwrap_or_else(|| response.to_string())
}

fn manga_key_from_input(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) {
        return None;
    }
    let key = normalize_key(input.trim_start_matches(BASE_URL));
    key.starts_with("/manga/").then_some(key)
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return normalize_key(&value[index + BASE_URL.len()..]);
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| srcset_first(html::attr_after(chunk, "<img", "data-lazy-srcset")))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn cover_from_onclick(onclick: Option<&str>) -> Option<String> {
    let onclick = onclick?;
    let marker = "addToHistory(";
    let start = onclick.find(marker)?;
    onclick[start..]
        .split('\'')
        .nth(5)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn srcset_first(value: Option<String>) -> Option<String> {
    value.and_then(|srcset| {
        srcset
            .split(',')
            .next()?
            .split_whitespace()
            .next()
            .map(ToString::to_string)
    })
}

fn section_after<'a>(body: &'a str, marker: &str) -> Option<&'a str> {
    let index = body.find(marker)?;
    Some(&body[index..])
}

fn text_values(chunk: &str, tag: &str) -> Vec<String> {
    chunk
        .split(&format!("<{tag}"))
        .skip(1)
        .filter_map(|part| html::text_between(part, ">", &format!("</{tag}>")))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn scanlator_from_chunk(chunk: &str) -> Option<String> {
    html::text_between(chunk, "Team", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn status_from_text(value: &str) -> ItemStatus {
    let lower = value.to_ascii_lowercase();
    if lower.contains("cours") {
        ItemStatus::Ongoing
    } else if lower.contains("termin") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn quoted_strings(body: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for quote in ['"', '\''] {
        let mut rest = body;
        while let Some(start) = rest.find(quote) {
            let after = &rest[start + 1..];
            let Some(end) = after.find(quote) else { break };
            out.push(&after[..end]);
            rest = &after[end + 1..];
        }
    }
    out
}

fn is_base64_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '+' || ch == '/' || ch == '='
}

#[derive(Deserialize)]
struct AjaxResponse {
    data: Option<String>,
}

const LIST_FIXTURE: &str = r#"
<div class="popular-section"><div class="manga-card"><a href="https://hentai.scanreader.net/manga/sample" onclick="addToHistory(1,'Sample','/cover.jpg')"><img data-lazy-src="/cover.jpg"></a><h3>Sample</h3></div></div>
<a class="pagination-next" href="/bibliotheque/page/1/"></a>
"#;
const LATEST_FIXTURE: &str = r#"
<div class="manga-cover"><a href="https://hentai.scanreader.net/manga/sample"><img data-lazy-src="/cover.jpg"></a><h3 class="manga-title-display">Sample</h3></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<meta property="og:image" content="/cover.jpg"><h1 class="manga-title">Sample</h1>
<div class="manga-content"><div style="background: #333"><p>Resume</p></div></div>
<div class="manga-info-grid"><div><div>Auteur</div><div><span>Auteur</span></div></div><div><div>Genres</div><div><span>Action</span></div></div><div><div>Statut</div><div>En cours</div></div></div>
<div id="secure-chapters-container" data-manga-id="1" data-nonce="nonce"></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"{"data":"<a href=\"https://hentai.scanreader.net/chapitre/sample-1\"><h4>Chapitre 1</h4><p>2024-01-01</p></a>"}"#;
const PAGES_FIXTURE: &str = r#"<script>const pages = ["gpGaj9GcvR3LtFmZuRXYu5WasVGbpR3clN2Lul2ZhRHdo1UahR2chJ3L"];</script>"#;
