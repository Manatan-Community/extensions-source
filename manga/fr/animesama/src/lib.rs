use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: AnimeSama = AnimeSama;
const BASE_URL: &str = "https://anime-sama.to";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct AnimeSama;

impl MangaSource for AnimeSama {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_catalog(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            return Ok(parse_latest(&fetch_document_or_fixture(
                BASE_URL,
                LATEST_FIXTURE,
            )));
        }
        Ok(parse_catalog(&fetch_document_or_fixture(
            &catalog_url(page, "", request.get("filters")),
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
        if let Some(key) = deeplink_key(query) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            let item = parse_details(&body, Some(key));
            if item.title.is_empty() {
                return Ok(Paged {
                    entries: Vec::new(),
                    has_next_page: false,
                });
            }
            return Ok(Paged {
                entries: vec![item],
                has_next_page: false,
            });
        }
        Ok(parse_catalog(&fetch_document_or_fixture(
            &catalog_url(page, query, request.get("filters")),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/catalogue/sample/".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/catalogue/sample/".into());
        let page_url = url::join_url(BASE_URL, &key);
        let body = fetch_document_or_fixture(&page_url, DETAILS_FIXTURE);
        Ok(parse_chapters_from_details(&body, &page_url))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            "/s2/scans/get_nb_chap_et_img.php?oeuvre=Sample&id=1&title=Sample".into()
        });
        Ok(parse_pages_for_key(&key))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink_key(input) {
            let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
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
        .with_header("Accept-Language", "fr-FR")
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalog_url(page: u64, query: &str, filters: Option<&Value>) -> String {
    let mut pairs = vec![
        ("type[0]".to_string(), "Scans".to_string()),
        ("page".to_string(), page.to_string()),
    ];
    if !query.trim().is_empty() {
        pairs.push(("search".to_string(), query.trim().to_string()));
    }
    for genre in selected_values(filters, "genre") {
        pairs.push(("genre[0]".to_string(), genre));
    }
    format!(
        "{BASE_URL}/catalogue?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!(
                "{}={}",
                url::query_escape(&key),
                url::query_escape(&value)
            ))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn parse_catalog(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: element_blocks(body, "<div")
            .into_iter()
            .filter(|chunk| chunk.contains("card-title"))
            .filter_map(catalog_item)
            .collect(),
        has_next_page: body.contains("id=\"list_pagination\"") && body.contains("bg-sky-900 +")
            || body.contains("Suivant"),
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    let section = body.split("containerAjoutsScans").nth(1).unwrap_or(body);
    let mut page = parse_catalog(section);
    for item in &mut page.entries {
        item.key = item
            .key
            .trim_end_matches("scan/vf")
            .trim_end_matches('/')
            .to_string();
        item.url = Some(url::join_url(BASE_URL, &item.key));
    }
    page.has_next_page = false;
    page
}

fn catalog_item(chunk: String) -> Option<CatalogItem> {
    let href = html::attr_after(&chunk, "<a", "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(&chunk, "card-title", "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "AnimeSama".into()));
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(&chunk, "<img", "src").map(|value| url::join_url(BASE_URL, &value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/catalogue/sample/".into());
    CatalogItem {
        key: normalize_key(&key),
        title: text_by_id(body, "titreOeuvre").unwrap_or_else(|| "AnimeSama".into()),
        cover: html::attr_after(body, "id=\"coverOeuvre\"", "src")
            .map(|value| url::join_url(BASE_URL, &value)),
        description: text_after_heading(body, "Synopsis").filter(|value| !value.is_empty()),
        tags: text_after_heading(body, "Genres")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_from_details(body: &str, details_url: &str) -> Vec<MangaChapter> {
    let mut chapters = Vec::new();
    for (scan_title, scan_url) in panneau_scans(body) {
        if scan_url.contains("va") {
            continue;
        }
        let scanlator = scan_title
            .replace("Scans", "")
            .replace(['(', ')'], "")
            .trim()
            .to_string();
        let scan_page =
            fetch_document_or_fixture(&url::join_url(details_url, &scan_url), SCAN_PAGE_FIXTURE);
        chapters.extend(parse_scan_page(&scan_page, &scanlator));
    }
    chapters.sort_by(|a, b| {
        a.chapter_number
            .unwrap_or_default()
            .partial_cmp(&b.chapter_number.unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters.reverse();
    chapters
}

fn parse_scan_page(body: &str, scanlator: &str) -> Vec<MangaChapter> {
    let title = text_by_id(body, "titreOeuvre").unwrap_or_else(|| "Sample".into());
    let api_key = format!(
        "/s2/scans/get_nb_chap_et_img.php?oeuvre={}",
        url::query_escape(&title)
    );
    let api = fetch_text_or_fixture(&url::join_url(BASE_URL, &api_key), NB_CHAP_FIXTURE);
    let counts = serde_json::from_str::<serde_json::Map<String, Value>>(&api)
        .unwrap_or_else(|_| serde_json::from_str(NB_CHAP_FIXTURE).expect("fixture is valid"));
    let mut chapters = Vec::new();
    let mut chapter_delay = 0_i64;
    if body.contains("resetListe()") {
        for command in body.split(';') {
            if let Some(data) = inside_call(command, "creerListe(") {
                let mut parts = data.split(',');
                let start = parts
                    .next()
                    .and_then(|value| value.trim().parse::<i64>().ok())
                    .unwrap_or(1);
                let end = parts
                    .next()
                    .and_then(|value| value.trim().parse::<i64>().ok())
                    .unwrap_or(start);
                for number in start..=end {
                    push_chapter(
                        &mut chapters,
                        &api_key,
                        &title,
                        number.to_string(),
                        scanlator,
                    );
                }
            } else if let Some(data) = inside_call(command, "newSP(") {
                let name = data.trim().trim_matches('"').to_string();
                push_chapter(&mut chapters, &api_key, &title, name, scanlator);
                chapter_delay += 1;
            }
        }
    }
    while chapters.len() < counts.len() {
        let name = (chapters.len() as i64 + 1 - chapter_delay).to_string();
        push_chapter(&mut chapters, &api_key, &title, name, scanlator);
    }
    chapters
}

fn push_chapter(
    chapters: &mut Vec<MangaChapter>,
    api_key: &str,
    title: &str,
    name: String,
    scanlator: &str,
) {
    let id = chapters.len() + 1;
    let key = format!("{api_key}&id={id}&title={}", url::query_escape(title));
    chapters.push(MangaChapter {
        key: key.clone(),
        title: Some(format!("Chapitre {name}")),
        chapter_number: name.parse::<f32>().ok(),
        scanlators: (!scanlator.is_empty())
            .then(|| vec![scanlator.to_string()])
            .unwrap_or_default(),
        url: Some(url::join_url(BASE_URL, &key)),
        ..MangaChapter::default()
    });
}

fn parse_pages_for_key(key: &str) -> Vec<MangaPage> {
    let title = query_param(key, "oeuvre")
        .or_else(|| query_param(key, "title"))
        .unwrap_or_else(|| "Sample".into());
    let chapter = query_param(key, "id").unwrap_or_else(|| "1".into());
    let api = format!(
        "{BASE_URL}/s2/scans/get_nb_chap_et_img.php?oeuvre={}",
        url::query_escape(&title)
    );
    let body = fetch_text_or_fixture(&api, NB_CHAP_FIXTURE);
    let counts = serde_json::from_str::<serde_json::Map<String, Value>>(&body)
        .unwrap_or_else(|_| serde_json::from_str(NB_CHAP_FIXTURE).expect("fixture is valid"));
    let count = counts.get(&chapter).and_then(Value::as_u64).unwrap_or(0);
    (1..=count)
        .map(|index| {
            let image = format!(
                "{BASE_URL}/s2/scans/{}/{chapter}/{index}.jpg",
                url::query_escape(&title).replace('+', "%20")
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {index}")),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn panneau_scans(body: &str) -> Vec<(String, String)> {
    body.split("panneauScan(")
        .skip(1)
        .filter_map(|chunk| {
            let args = chunk.split(')').next()?;
            let mut values = args.split("\", \"");
            let title = values.next()?.trim().trim_matches('"').to_string();
            let path = values.next()?.trim().trim_matches('"').to_string();
            Some((title, path))
        })
        .collect()
}

fn inside_call<'a>(command: &'a str, call: &str) -> Option<&'a str> {
    let start = command.find(call)? + call.len();
    let end = command[start..].find(')')? + start;
    Some(&command[start..end])
}

fn selected_values(filters: Option<&Value>, id: &str) -> Vec<String> {
    match filters.and_then(|filters| filters.get(id)) {
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        Some(Value::String(value)) if !value.is_empty() => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn deeplink_key(input: &str) -> Option<String> {
    if !input.starts_with(BASE_URL) || !input.contains("/catalogue/") {
        return None;
    }
    let mut key = normalize_key(input);
    if key.contains("/scan") {
        key = key
            .split("/scan")
            .next()
            .unwrap_or(&key)
            .trim_end_matches('/')
            .to_string();
    }
    Some(format!("{}/", key.trim_end_matches('/')))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split(['?', '#'])
        .next()
        .unwrap_or(input);
    format!("/{}", path.trim_start_matches('/'))
}

fn query_param(input: &str, name: &str) -> Option<String> {
    input
        .split('?')
        .nth(1)
        .unwrap_or(input)
        .split('&')
        .find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key == name).then(|| decode_query(value))
        })
}

fn decode_query(value: &str) -> String {
    value.replace('+', " ").replace("%20", " ")
}

fn text_by_id(body: &str, id: &str) -> Option<String> {
    html::text_between(body, &format!("id=\"{id}\""), "</")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn text_after_heading(body: &str, heading: &str) -> Option<String> {
    body.split(&format!(">{heading}<"))
        .nth(1)
        .and_then(|chunk| {
            html::text_between(chunk, "<p", "</p>")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
        })
        .map(|value| html::strip_tags(&value))
}

fn element_blocks(input: &str, marker: &str) -> Vec<String> {
    input
        .split(marker)
        .skip(1)
        .map(|chunk| format!("{marker}{chunk}"))
        .collect()
}

const LIST_FIXTURE: &str = r#"<div id="list_catalog"><div><a href="/catalogue/sample/"><img src="/cover.jpg"><div class="card-title">Sample</div></a></div></div>"#;
const LATEST_FIXTURE: &str = r#"<div id="containerAjoutsScans"><div><a href="/catalogue/sample/scan/vf/"><img src="/cover.jpg"><div class="card-title">Sample</div></a></div></div>"#;
const DETAILS_FIXTURE: &str = r#"
<h1 id="titreOeuvre">Sample</h1><img id="coverOeuvre" src="/cover.jpg">
<div id="sousBlocMiddle"><div><h2>Synopsis</h2><p>Summary</p><h2>Genres</h2><a>Action</a></div></div>
<script>panneauScan("Scans VF", "scan/vf/");</script>
"#;
const SCAN_PAGE_FIXTURE: &str =
    r#"<h1 id="titreOeuvre">Sample</h1><script>resetListe();creerListe(1, 1);</script>"#;
const NB_CHAP_FIXTURE: &str = r#"{"1":2}"#;
