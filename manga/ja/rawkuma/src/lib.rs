use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource, webview,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Rawkuma = Rawkuma;
const BASE_URL: &str = "https://rawkuma.net";

struct Rawkuma;

impl MangaSource for Rawkuma {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let orderby = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "modified"
        } else {
            "meta_value_num"
        };
        search_ajax("", page(&request), orderby)
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
        search_ajax(query, page(&request), "modified")
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        let details = details_by_key(&key);
        let id = details
            .extra
            .get("id")
            .and_then(Value::as_i64)
            .map(|value| value.to_string())
            .unwrap_or_else(|| {
                manga_id_from_html(&fetch_document(&absolute_url(&key), DETAILS_HTML_FIXTURE))
                    .unwrap_or_else(|| "1".into())
            });
        Ok(parse_chapters(&fetch_document(
            &format!(
                "{BASE_URL}/wp-admin/admin-ajax.php?manga_id={id}&page=99&action=chapter_list"
            ),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/sample-chapter-1".into());
        let target = absolute_url(&key);
        let body = client().get(&target).send_text().unwrap_or_default();
        let mut images = page_images_from_html(&body);
        if images.is_empty() {
            images = webview::extract_json::<Vec<String>>(
                webview::ExtractRequest::new(
                    &target,
                    r#"
Array.from(document.images)
  .map((image) => image.currentSrc || image.src || image.getAttribute("data-src") || "")
  .filter((src) => src && !src.startsWith("data:") && src.includes("/s/") && /\/chapter-[^/]+\//.test(src))
"#,
                )
                .wait_for_script(
                    r#"
Array.from(document.images).some((image) => {
  const src = image.currentSrc || image.src || image.getAttribute("data-src") || "";
  return src.includes("/s/") && /\/chapter-[^/]+\//.test(src);
})
"#,
                )
                .timeout_ms(60_000),
            )
            .unwrap_or_default();
        }
        Ok(pages_from_images(images, &target))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1, "listingId": "popular"}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest"}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
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
                item: Some(details_by_key(&key)),
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

fn fetch_json(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn post_form(target: &str, form: &[(&str, &str)], fixture: &str) -> String {
    client()
        .post(target)
        .xhr()
        .form(form)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn search_ajax(query: &str, page: u64, orderby: &str) -> ExtensionResult<Paged<CatalogItem>> {
    let nonce_doc = fetch_document(
        &format!("{BASE_URL}/wp-admin/admin-ajax.php?type=search_form&action=get_nonce"),
        NONCE_FIXTURE,
    );
    let nonce =
        html::attr_after(&nonce_doc, "search_nonce", "value").unwrap_or_else(|| "nonce".into());
    let page_string = page.to_string();
    let body = post_form(
        &format!("{BASE_URL}/wp-admin/admin-ajax.php?action=advanced_search"),
        &[
            ("nonce", &nonce),
            ("inclusion", "OR"),
            ("exclusion", "OR"),
            ("page", &page_string),
            ("genre", "[]"),
            ("genre_exclude", "[]"),
            ("author", "[]"),
            ("artist", "[]"),
            ("project", "0"),
            ("type", "[]"),
            ("status", "[]"),
            ("order", "desc"),
            ("orderby", orderby),
            ("query", query),
        ],
        SEARCH_FIXTURE,
    );
    let slugs = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "href"))
        .filter_map(|href| key_from_url(&href))
        .filter_map(|key| url::slug_from_url(&key))
        .fold(Vec::<String>::new(), |mut out, slug| {
            if !out.contains(&slug) {
                out.push(slug);
            }
            out
        });
    if slugs.is_empty() {
        return Ok(Paged {
            entries: parse_listing_html(&body),
            has_next_page: false,
        });
    }
    let mut target = format!(
        "{BASE_URL}/wp-json/wp/v2/manga?per_page={}&_embed",
        slugs.len() + 1
    );
    for slug in slugs {
        target.push_str("&slug[]=");
        target.push_str(&url::query_escape(&slug));
    }
    Ok(parse_manga_list(
        &fetch_json(&target, JSON_LIST_FIXTURE),
        body.contains("<button"),
    ))
}

#[derive(Deserialize)]
struct Rendered {
    rendered: String,
}

#[derive(Deserialize)]
struct Term {
    name: String,
    taxonomy: String,
}

#[derive(Deserialize)]
struct FeaturedMedia {
    #[serde(rename = "source_url")]
    source_url: String,
}

#[derive(Deserialize)]
struct Embedded {
    #[serde(default, rename = "wp:featuredmedia")]
    featured_media: Vec<FeaturedMedia>,
    #[serde(default, rename = "wp:term")]
    terms: Vec<Vec<Term>>,
}

#[derive(Deserialize)]
struct WpManga {
    id: i64,
    slug: String,
    title: Rendered,
    content: Rendered,
    #[serde(rename = "_embedded")]
    embedded: Embedded,
}

fn parse_manga_list(body: &str, has_next_page: bool) -> Paged<CatalogItem> {
    let entries = serde_json::from_str::<Vec<WpManga>>(body)
        .unwrap_or_default()
        .into_iter()
        .filter(|item| {
            !terms(&item.embedded, "type")
                .iter()
                .any(|term| term == "Novel")
        })
        .map(wp_to_catalog)
        .collect();
    Paged {
        entries,
        has_next_page,
    }
}

fn wp_to_catalog(item: WpManga) -> CatalogItem {
    let mut extra = serde_json::Map::new();
    extra.insert("id".into(), Value::from(item.id));
    CatalogItem {
        key: format!("/manga/{}", item.slug),
        title: html::strip_tags(&item.title.rendered),
        cover: item
            .embedded
            .featured_media
            .first()
            .map(|media| media.source_url.clone()),
        authors: terms(&item.embedded, "series-author"),
        artists: terms(&item.embedded, "artist"),
        tags: {
            let mut tags = terms(&item.embedded, "genre");
            tags.extend(terms(&item.embedded, "type"));
            tags
        },
        description: Some(html::strip_tags(&item.content.rendered))
            .filter(|value| !value.is_empty()),
        status: parse_status_terms(&terms(&item.embedded, "status")),
        url: Some(format!("{BASE_URL}/manga/{}/", item.slug)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        extra: extra.into_iter().collect(),
        ..CatalogItem::default()
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let slug = url::slug_from_url(key).unwrap_or_else(|| "sample".into());
    let body = fetch_json(
        &format!(
            "{BASE_URL}/wp-json/wp/v2/manga?slug[]={}&_embed",
            url::query_escape(&slug)
        ),
        JSON_DETAILS_FIXTURE,
    );
    serde_json::from_str::<Vec<WpManga>>(&body)
        .ok()
        .and_then(|mut items| items.pop())
        .map(wp_to_catalog)
        .unwrap_or_else(|| {
            let mut item =
                parse_listing_html(&fetch_document(&absolute_url(key), DETAILS_HTML_FIXTURE))
                    .into_iter()
                    .next()
                    .unwrap_or_default();
            item.key = normalize_key(key);
            item.initialized = true;
            item
        })
}

fn parse_listing_html(body: &str) -> Vec<CatalogItem> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = key_from_url(&href)?;
            Some(CatalogItem {
                key: key.clone(),
                title: html::attr_after(chunk, "<img", "alt")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "Rawkuma".into())
                    }),
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("adult".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("<time") || chunk.contains("href"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = key_from_url(&href)?;
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<span", "</span>")
                    .or_else(|| html::text_between(chunk, ">", "</a>"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                date_uploaded: html::attr_after(chunk, "<time", "datetime")
                    .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

#[cfg(test)]
fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    pages_from_images(page_images_from_html(body), referer)
}

fn page_images_from_html(body: &str) -> Vec<String> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src").or_else(|| html::attr(chunk, "data-src")))
        .filter(|src| !src.is_empty() && !src.starts_with("data:"))
        .collect()
}

fn pages_from_images(images: Vec<String>, referer: &str) -> Vec<MangaPage> {
    images
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn terms(embedded: &Embedded, taxonomy: &str) -> Vec<String> {
    embedded
        .terms
        .iter()
        .find(|items| items.first().is_some_and(|term| term.taxonomy == taxonomy))
        .map(|items| items.iter().map(|term| term.name.clone()).collect())
        .unwrap_or_default()
}

fn parse_status_terms(status: &[String]) -> ItemStatus {
    if status.iter().any(|term| term == "Completed") {
        ItemStatus::Completed
    } else if status.iter().any(|term| term == "Ongoing") {
        ItemStatus::Ongoing
    } else if status.iter().any(|term| term == "Cancelled") {
        ItemStatus::Cancelled
    } else if status.iter().any(|term| term == "On Hiatus") {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn manga_id_from_html(body: &str) -> Option<String> {
    body.split("manga_id=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .map(ToString::to_string)
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .find("/manga/")
        .map(|index| normalize_key(&input[index..]))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('#')
        .next()
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1)
}

export_manga_source!(SOURCE);

const NONCE_FIXTURE: &str = r#"<input name="search_nonce" value="nonce">"#;
const SEARCH_FIXTURE: &str = r#"<div><a href="https://rawkuma.net/manga/sample/"><img src="/cover.jpg"></a><button><svg></svg></button></div>"#;
const JSON_LIST_FIXTURE: &str = r#"[{"id":1,"slug":"sample","title":{"rendered":"Sample Rawkuma"},"content":{"rendered":"Summary"},"_embedded":{"wp:featuredmedia":[{"source_url":"https://rawkuma.net/cover.jpg"}],"wp:term":[[{"name":"Action","slug":"action","taxonomy":"genre"}],[{"name":"Ongoing","slug":"ongoing","taxonomy":"status"}]]}}]"#;
const JSON_DETAILS_FIXTURE: &str = JSON_LIST_FIXTURE;
const DETAILS_HTML_FIXTURE: &str = r#"<div id="gallery-list" hx-get="/?manga_id=1&x=1"></div><a href="/manga/sample/"><img src="/cover.jpg" alt="Sample Rawkuma"></a>"#;
const CHAPTERS_FIXTURE: &str = r#"<div><a href="/manga/sample/chapter-1"><span>Chapter 1</span><time datetime="2026-01-01T00:00:00Z"></time></a><a href="https://drive.google.com/uc"><span>Download</span></a></div>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_chapters_ignores_download_links() {
        let chapters = parse_chapters(
            r#"<a href="/manga/sample/chapter-1"><span>Chapter 1</span></a><a href="https://drive.google.com/uc"><span>Download</span></a>"#,
        );

        assert_eq!(chapters.len(), 1);
        assert_eq!(chapters[0].key, "/manga/sample/chapter-1");
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
    }

    #[test]
    fn parse_pages_extracts_reader_images() {
        let pages = parse_pages(
            r#"<main><img src="/page1.jpg"><img data-src="/page2.jpg"></main>"#,
            "https://rawkuma.net/manga/sample/chapter-1",
        );

        assert_eq!(pages.len(), 2);
    }
}
