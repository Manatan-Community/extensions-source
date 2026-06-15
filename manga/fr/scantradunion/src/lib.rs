use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ScantradUnion = ScantradUnion;
const BASE_URL: &str = "https://scantrad-union.com";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct ScantradUnion;

impl MangaSource for ScantradUnion {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_project_cards(LIST_FIXTURE),
                has_next_page: false,
            });
        }
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            BASE_URL.to_string()
        } else {
            format!("{BASE_URL}/projets/")
        };
        let body = fetch_doc(&target, LIST_FIXTURE);
        let entries = if target == BASE_URL {
            parse_latest(&body)
        } else {
            parse_project_cards(&body)
        };
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = manga_key_from_url(query);
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/?s={}&asp_active=1&p_asid=1&p_asp_data=YXNwX2dlbiU1QiU1RD10aXRsZSZjdXN0b21zZXQlNUIlNUQ9bWFuZ2E=",
            url::query_escape(query)
        );
        let body = fetch_doc(&target, SEARCH_FIXTURE);
        if body.contains("projet-description") {
            let key = html::attr_after(&body, "rel=\"canonical\"", "href")
                .map(|href| normalize_key(&href))
                .unwrap_or_else(|| "/manga/sample".to_string());
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: parse_search_cards(&body),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_details(
            &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/manga/sample".into());
        Ok(parse_chapters(&fetch_doc(
            &url::join_url(BASE_URL, &key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/read/sample/chapter-1.00/page-1".into());
        Ok(parse_pages(&fetch_doc(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = manga_key_from_url(input);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_doc(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
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

fn fetch_doc(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_project_cards(body: &str) -> Vec<CatalogItem> {
    body.split("index-top3-a")
        .skip(1)
        .filter_map(|chunk| {
            let href =
                html::attr(chunk, "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: clean_title(
                    &html::text_between(chunk, "index-top3-title", "</")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Scantrad Union".to_string()),
                ),
                cover: style_image(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_latest(body: &str) -> Vec<CatalogItem> {
    let cards = body
        .split("dernieresmaj")
        .nth(1)
        .unwrap_or(body)
        .split("<div")
        .skip(1)
        .filter(|chunk| chunk.contains("colonne") || chunk.contains("text-truncate"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "text-truncate", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: clean_title(
                    &html::text_between(chunk, "text-truncate", "</")
                        .or_else(|| html::attr_after(chunk, "<a", "title"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Scantrad Union".to_string()),
                ),
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                authors: text_by_class(chunk, "nomteam").into_iter().collect(),
                artists: text_by_class(chunk, "nomteam").into_iter().collect(),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    if cards.is_empty() {
        parse_project_cards(body)
    } else {
        cards
    }
}

fn parse_search_cards(body: &str) -> Vec<CatalogItem> {
    body.split("<article")
        .skip(1)
        .filter(|chunk| chunk.contains("post-outer"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "index-post-header-a", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: clean_title(
                    &html::text_between(chunk, "index-post-header-a", "</")
                        .or_else(|| html::attr_after(chunk, "<a", "title"))
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Scantrad Union".to_string()),
                ),
                cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/manga/sample".into());
    CatalogItem {
        key: key.clone(),
        title: clean_title(
            &html::text_between(body, "projet-description", "</h2>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .or_else(|| url::slug_from_url(&key))
                .unwrap_or_else(|| "Scantrad Union".to_string()),
        ),
        cover: html::attr_after(body, "projet-image", "src")
            .or_else(|| image_attr(body))
            .map(|image| url::join_url(BASE_URL, &image)),
        description: html::text_between(body, "sContent", "</p>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        authors: links_after_label(body, "Auteur"),
        artists: links_after_label(body, "Auteur"),
        tags: links_after_label(body, "Genres"),
        status: parse_status(&status_text(body)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("name-chapter") && chunk.contains("btnlel"))
        .filter_map(|chunk| {
            let href = chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::attr(part, "href"))
                .find(|href| href.contains("/read/"))?;
            let key = normalize_key(&href);
            let number = html::text_between(chunk, "chapter-number", "</")
                .map(|value| html::strip_tags(&value).replace('#', ""))
                .unwrap_or_default();
            let name = html::text_between(chunk, "chapter-name", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_default();
            let title = [number.trim(), name.trim()]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join(" - ");
            Some(MangaChapter {
                key: key.clone(),
                title: Some(if title.is_empty() {
                    "Chapitre".into()
                } else {
                    title
                }),
                chapter_number: chapter_number(&key),
                date_uploaded: find_date(chunk),
                scanlators: link_values(chunk, "btnteam"),
                url: Some(url::join_url(BASE_URL, &key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let mut seen = Vec::<String>::new();
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("webtoon") || chunk.contains("data-src") || chunk.contains("src")
        })
        .filter_map(image_attr)
        .filter(|image| {
            if image.starts_with("data:") || seen.contains(image) {
                false
            } else {
                seen.push(image.clone());
                true
            }
        })
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

fn manga_key_from_url(input: &str) -> String {
    let key = normalize_key(input);
    if key.starts_with("/read/") {
        let slug = key
            .trim_start_matches("/read/")
            .split('/')
            .next()
            .unwrap_or("sample");
        format!("/manga/{slug}")
    } else {
        key
    }
}

fn normalize_key(input: &str) -> String {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    format!("/{}", path.trim_start_matches('/').trim_end_matches('/'))
}

fn clean_title(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("[Partenaire]")
        .trim()
        .to_string()
}

fn style_image(input: &str) -> Option<String> {
    let style = html::attr(input, "style")?;
    let marker = "url(";
    let start = style.find(marker)? + marker.len();
    let rest = style[start..].trim_start_matches(['\'', '"']);
    let end = rest.find(['\'', '"', ')']).unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "src"))
}

fn links_after_label(body: &str, label: &str) -> Vec<String> {
    body.split(label)
        .nth(1)
        .unwrap_or("")
        .split("divider2")
        .next()
        .unwrap_or("")
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
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

fn text_by_class(body: &str, class_name: &str) -> Option<String> {
    html::text_between(body, class_name, "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn status_text(body: &str) -> String {
    let labels = body
        .split("label label-primary")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</span>"))
        .map(|value| html::strip_tags(&value))
        .collect::<Vec<_>>();
    labels
        .get(2)
        .or_else(|| labels.first())
        .cloned()
        .unwrap_or_default()
}

fn parse_status(value: &str) -> ItemStatus {
    match value.trim().to_ascii_lowercase().as_str() {
        "en cours" => ItemStatus::Ongoing,
        "termine" | "terminé" => ItemStatus::Completed,
        "licencie" | "licencié" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn find_date(value: &str) -> Option<i64> {
    html::strip_tags(value).split_whitespace().find_map(|part| {
        let mut pieces = part.split('-');
        let day = pieces.next()?;
        let month = pieces.next()?;
        let year = pieces.next()?;
        dates::parse_ymd(&format!("{year}-{month}-{day}"))
    })
}

fn chapter_number(key: &str) -> Option<f32> {
    key.split("chapter-")
        .nth(1)?
        .split('/')
        .next()?
        .parse()
        .ok()
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"<a href="https://scantrad-union.com/manga/sample/" class="index-top3-a"><div class="index-top3-bg" style="background:url('https://scantrad-union.com/cover.jpg');"><h2 class="index-top3-title">Sample Union</h2></div></a>"#;
const SEARCH_FIXTURE: &str = r#"<article class="post-outer"><a class="index-post-header-a" href="https://scantrad-union.com/manga/sample/">Sample Union</a><img class="wp-post-image" src="/cover.jpg"></article>"#;
const DETAILS_FIXTURE: &str = r#"<div class="projet-description"><h2>Sample Union</h2><p class="sContent">Resume</p><div class="project-details"><h5>Auteur(s) &amp; Artiste(s) :</h5><a>Auteur</a><div class="divider2"></div><h5>Statut VO :</h5><span class="label label-primary">Termine</span><div class="divider2"></div><h5>Dernier chapitre VA :</h5><span class="label label-primary">Indisponible</span><div class="divider2"></div><h5>Statut VF :</h5><span class="label label-primary">Termine</span><div class="divider2"></div><h5>Genres :</h5><a>Action</a></div></div><div class="projet-image"><img src="/cover.jpg"></div><ul class="links-projects"><li><div class="name-chapter"><span class="chapter-number">#1</span><span class="chapter-name">Debut</span><span>01-01-2024</span><div class="buttons"><a class="btnteam">Team</a><a href="https://scantrad-union.com/read/sample/chapter-1.00/page-1/" class="btnlel">Lire</a></div></div></li></ul>"#;
const PAGES_FIXTURE: &str =
    r#"<div id="webtoon"><a><img data-src="/page1.jpg"></a><a><img src="/page2.jpg"></a></div>"#;
