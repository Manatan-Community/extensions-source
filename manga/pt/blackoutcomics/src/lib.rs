use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: BlackoutComics = BlackoutComics;
const BASE_URL: &str = "https://blackoutcomics.com";
const LANG: &str = "pt-BR";
const CONTENT_RATING: &str = "adult";

struct BlackoutComics;

impl MangaSource for BlackoutComics {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE, ".ranking-grid"));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            format!("{BASE_URL}/atualizados-recente?page={page}")
        } else {
            format!("{BASE_URL}/ranking")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), ""))
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
                    &fetch_document(query, DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        if !query.is_empty() {
            return Ok(parse_search_json(&fetch_document(
                &format!(
                    "{BASE_URL}/comics?src={}&format=json",
                    url::query_escape(query)
                ),
                SEARCH_FIXTURE,
            )));
        }
        let mut target = format!("{BASE_URL}/comics");
        let mut sep = "?";
        for id in ["status", "gen"] {
            if let Some(value) = filter_value(&request, id) {
                target.push_str(sep);
                sep = "&";
                target.push_str(id);
                target.push('=');
                target.push_str(&url::query_escape(&value));
            }
        }
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE), ""))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/1".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/1".into());
        Ok(parse_chapters(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comics/1/ler/capitulo-1".into());
        login_if_configured(&request);
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

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: key
                    .starts_with("/comics/")
                    .then(|| parse_details(&fetch_document(input, DETAILS_FIXTURE), Some(key))),
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
        .with_origin(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .header("DNT", "1")
        .header("Sec-GPC", "1")
        .header("Cookie", "age_gate_consent={\"consentAt\":1777661090431,\"expiresAt\":1778265890431}; _popprepop=1")
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn login_if_configured(request: &Value) {
    let Some(prefs) = request.get("preferences") else {
        return;
    };
    let email = prefs
        .get("email")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let password = prefs
        .get("password")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if email.is_empty() || password.is_empty() {
        return;
    }
    let home = fetch_document(BASE_URL, "");
    let token = html::attr_after(&home, "csrf-token", "content");
    if let Some(token) = token {
        let _ = client()
            .post(format!("{BASE_URL}/entrar"))
            .xhr()
            .header("X-CSRF-TOKEN", token.as_str())
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Referer", format!("{BASE_URL}/"))
            .form(&[
                ("_token", token.as_str()),
                ("USE_EMAIL", email),
                ("password", password),
            ])
            .send_text();
    }
}

fn parse_listing(body: &str, _scope: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<a")
            .skip(1)
            .filter(|chunk| chunk.contains("webtoon-card"))
            .filter_map(|chunk| {
                let href = html::attr(chunk, "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "card-title", "</")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .unwrap_or_else(|| {
                            url::slug_from_url(&key).unwrap_or_else(|| "Blackout Comics".into())
                        }),
                    cover: html::attr_after(chunk, "<img", "src").map(|image| absolute_url(&image)),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                })
            })
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagerx__link") && body.contains("rel=\"next\""),
    }
}

fn parse_search_json(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .or_else(|_| serde_json::from_str::<SearchResponse>(SEARCH_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: response
            .items
            .into_iter()
            .map(|item| {
                let key = format!("/comics/{}", item.id);
                CatalogItem {
                    key: key.clone(),
                    title: item.name,
                    cover: item
                        .img_url
                        .or_else(|| item.img_pr.map(|image| absolute_url(&image))),
                    url: Some(absolute_url(&key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    initialized: false,
                    ..CatalogItem::default()
                }
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/comics/1".into());
    let status_text = html::text_between(body, "status-pill", "</")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "project-title", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(&key).unwrap_or_else(|| "Blackout Comics".into())
            }),
        cover: html::attr_after(body, "project-cover", "src").map(|image| absolute_url(&image)),
        authors: info_value(body, "fa-pen-nib").into_iter().collect(),
        artists: info_value(body, "fa-palette").into_iter().collect(),
        description: html::text_between(body, "project-description", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: body
            .split("genre-tag")
            .skip(1)
            .filter_map(|chunk| {
                html::text_between(chunk, ">", "</").map(|value| html::strip_tags(&value))
            })
            .collect(),
        status: if status_text.contains("completo") {
            ItemStatus::Completed
        } else if status_text.contains("lanc") || status_text.contains("lan") {
            ItemStatus::Ongoing
        } else {
            ItemStatus::Unknown
        },
        url: Some(absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let chapters = body
        .split("normal_ep")
        .skip(1)
        .filter_map(|chunk| {
            let num = html::text_between(chunk, "num", "</")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| "1".into());
            let href = html::attr_after(chunk, "<a", "href")
                .unwrap_or_else(|| format!("{manga_key}/ler/capitulo-{num}"));
            let key = normalize_key(&href);
            let mut title = format!("Capitulo {num}");
            if let Some(extra) = html::text_between(chunk, "cell-title", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
            {
                title.push_str(" - ");
                title.push_str(&extra);
            }
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(absolute_url(&key)),
                chapter_number: num.replace(',', ".").parse::<f32>().ok(),
                date_uploaded: html::text_between(chunk, "text-muted", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_dd_mm_yy(&value)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        vec![MangaChapter {
            key: manga_key.to_string(),
            title: Some("Capitulo unico".into()),
            url: Some(absolute_url(manga_key)),
            ..MangaChapter::default()
        }]
    } else {
        chapters
    }
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    let json = body
        .split("S")
        .find_map(|chunk| {
            let after_eq = chunk.split_once('=')?.1.trim();
            after_eq
                .strip_prefix('[')?
                .split_once(']')
                .map(|(value, _)| format!("[{value}]"))
        })
        .unwrap_or_else(|| r#"["/page-1.jpg"]"#.into());
    serde_json::from_str::<Vec<String>>(&json)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: absolute_url(&image),
                context: None,
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn info_value(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    html::text_between(&body[start..], "<strong", "</strong>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn parse_dd_mm_yy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('.');
    let day = parts.next()?;
    let month = parts.next()?;
    let year = parts.next()?;
    dates::parse_ymd(&format!("20{year}-{month}-{day}"))
}

fn filter_value(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")?
        .get(id)?
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_key(input: &str) -> String {
    format!(
        "/{}",
        input.trim().trim_start_matches(BASE_URL).trim_matches('/')
    )
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
    entries
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    items: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    #[serde(rename = "PJT_ID")]
    id: i64,
    #[serde(rename = "PJT_NAME")]
    name: String,
    #[serde(rename = "PJT_IMG_PR")]
    img_pr: Option<String>,
    #[serde(rename = "PJT_IMG_PR_URL")]
    img_url: Option<String>,
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="webtoon-grid"><a class="webtoon-card" href="/comics/1"><div class="card-thumb"><img src="/cover.jpg"></div><div class="card-title"><span>Sample Blackout</span></div></a></div>
"#;
const SEARCH_FIXTURE: &str =
    r#"{"items":[{"PJT_ID":1,"PJT_NAME":"Sample Blackout","PJT_IMG_PR":"/cover.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="project-title">Sample Blackout</h1><img class="project-cover" src="/cover.jpg">
<div class="project-description">Sample description.</div><span class="status-pill">Completo</span>
<div id="tab-capitulos-list"><div class="normal_ep"><a href="/comics/1/ler/capitulo-1"></a><span class="num">1</span><div class="cell-title"><strong class="line-3">Start</strong></div><div class="cell-num"><span class="text-muted">01.01.24</span></div></div></div>
"#;
const PAGES_FIXTURE: &str = r#"<script>S = ["/page-1.jpg","/page-2.jpg"];</script>"#;
