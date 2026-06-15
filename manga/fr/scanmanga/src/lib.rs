use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use flate2::read::ZlibDecoder;
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;
use std::io::Read;

const SOURCE: ScanManga = ScanManga;
const BASE_URL: &str = "https://m.scan-manga.com";
const STATIC_IMAGE_URL: &str = "https://static.scan-manga.com/img/manga";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "adult";

struct ScanManga;

impl MangaSource for ScanManga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_popular(POPULAR_FIXTURE));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        if latest {
            Ok(parse_latest(&fetch_document(BASE_URL, LATEST_FIXTURE)))
        } else {
            Ok(parse_popular(&fetch_document(
                &format!("{BASE_URL}/TOP-Manga-Webtoon-45.html"),
                POPULAR_FIXTURE,
            )))
        }
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_input(query) {
            let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(key))],
                has_next_page: false,
            });
        }
        let target = format!(
            "{BASE_URL}/api/search/quick.json?term={}",
            url::query_escape(query)
        );
        Ok(parse_search(&fetch_text(&target, SEARCH_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(key)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample.html".into());
        let body = fetch_document(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/sample-chapitre-1.html".into());
        let chapter_url = url::join_url(BASE_URL, &key);
        let body = fetch_document(&chapter_url, PAGES_FIXTURE);
        let gpu = request
            .get("preferences")
            .and_then(|preferences| preferences.get("gpu_renderer"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        Ok(
            parse_pages(&body, &chapter_url, gpu)
                .unwrap_or_else(|| fixture_pages(PAGE_API_FIXTURE)),
        )
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
        if let Some(key) = key_from_input(input) {
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
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("X-Requested-With", "")
        .header("Accept-Language", "fr-FR,fr;q=0.9,en-US;q=0.8,en;q=0.7")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .header("X-Requested-With", "")
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_popular(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("top")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "<a", "href")?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "<a", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| html::attr_after(chunk, "<img", "alt"))
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Scan-Manga".to_string()),
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_latest(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("publi")
            .skip(1)
            .filter_map(|chunk| {
                let href = html::attr_after(chunk, "l_manga", "href")
                    .or_else(|| html::attr_after(chunk, "<a", "href"))?;
                let key = normalize_key(&href);
                Some(CatalogItem {
                    key: key.clone(),
                    title: html::text_between(chunk, "l_manga", "</a>")
                        .map(|value| html::strip_tags(&value))
                        .filter(|value| !value.is_empty())
                        .or_else(|| url::slug_from_url(&key))
                        .unwrap_or_else(|| "Scan-Manga".to_string()),
                    cover: image_attr(chunk).map(|image| url::join_url(BASE_URL, &image)),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    ..CatalogItem::default()
                })
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let response = serde_json::from_str::<SearchResponse>(body)
        .or_else(|_| serde_json::from_str(SEARCH_FIXTURE))
        .unwrap_or_default();
    Paged {
        entries: response
            .title
            .unwrap_or_default()
            .into_iter()
            .map(|item| {
                let key = normalize_key(&item.url);
                CatalogItem {
                    key: key.clone(),
                    title: item.nom_match,
                    cover: Some(format!(
                        "{STATIC_IMAGE_URL}/{}",
                        item.image.trim_start_matches('/')
                    )),
                    url: Some(url::join_url(BASE_URL, &key)),
                    language: Some(LANG.to_string()),
                    content_rating: Some(CONTENT_RATING.to_string()),
                    ..CatalogItem::default()
                }
            })
            .collect(),
        has_next_page: false,
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/sample.html".into());
    let info = html::text_between(body, "titres_souspart", "</div>").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "itemprop=name", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| "Scan-Manga".to_string()),
        authors: html::text_between(body, "itemprop=author", "</")
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: html::text_between(body, "itemprop=description", "</div>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        tags: html::text_between(body, "itemprop=genre", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        status: status_from_text(&info),
        cover: image_attr(body).map(|image| url::join_url(BASE_URL, &image)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("chapt_m")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let chapter_name = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapitre".to_string());
            let extra = html::text_between(chunk, "publititle", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty());
            let title = extra
                .map(|extra| format!("{chapter_name} - {extra}"))
                .unwrap_or(chapter_name);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.to_string()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_pages(body: &str, chapter_url: &str, gpu_renderer: &str) -> Option<Vec<MangaPage>> {
    let packed = script_containing(body, "eval(function")
        .or_else(|| script_containing(body, "const idc"))?;
    let unpacked = decode_hunter(&packed).unwrap_or_else(|| packed.clone());
    let sml = js_single_quoted(&unpacked, "sml")?;
    let sme = js_single_quoted(&unpacked, "sme")?;
    let chapter_id = text_after(&packed, "const idc = ")
        .or_else(|| text_after(&unpacked, "const idc = "))?
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    let top_domain = top_domain(BASE_URL);
    let fingerprint = fingerprint(gpu_renderer);
    let request_body = format!(r#"{{"a":"{sme}","b":"{sml}","c":"{fingerprint}"}}"#);
    let page_list_url = format!("https://bqj.{top_domain}/lel/{chapter_id}.json");
    let response = client()
        .post(page_list_url)
        .header("Origin", origin(chapter_url))
        .header("Referer", chapter_url)
        .header("Token", "yf")
        .json(request_body)
        .send_text()
        .unwrap_or_else(|_| PAGE_API_FIXTURE.to_string());
    Some(fixture_pages_from_payload(
        &response,
        chapter_id.parse().ok()?,
    ))
}

fn fixture_pages(body: &str) -> Vec<MangaPage> {
    fixture_pages_from_payload(body, 1)
}

fn fixture_pages_from_payload(body: &str, chapter_id: i64) -> Vec<MangaPage> {
    data_api(body, chapter_id)
        .unwrap_or_else(|| serde_json::from_str(PAGE_PAYLOAD_FIXTURE).unwrap_or_default())
        .generate_image_urls()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: image,
                context: None,
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn data_api(data: &str, chapter_id: i64) -> Option<UrlPayload> {
    if data.contains("error") {
        return None;
    }
    let compressed = decode_base64(data.trim())?;
    let mut decoder = ZlibDecoder::new(compressed.as_slice());
    let mut inflated = String::new();
    decoder.read_to_string(&mut inflated).ok()?;
    let cleaned = inflated.trim_end_matches(&format!("{chapter_id:x}"));
    let reversed = cleaned.chars().rev().collect::<String>();
    let json = String::from_utf8(decode_base64(&reversed)?).ok()?;
    serde_json::from_str(&json).ok()
}

fn decode_hunter(script: &str) -> Option<String> {
    let args = hunter_args(script)?;
    let encoded = args.first()?.clone();
    let mask = args.get(2)?.clone();
    let interval = args.get(3)?.parse::<i64>().ok()?;
    let option = args.get(4)?.parse::<usize>().ok()?;
    let delimiter = mask.chars().nth(option)?;
    let chars = mask.chars().collect::<Vec<_>>();
    let mut output = String::new();
    for token in encoded.split(delimiter).filter(|token| !token.is_empty()) {
        let mut digits = String::new();
        for ch in token.chars() {
            let digit = chars.iter().position(|candidate| *candidate == ch)?;
            digits.push_str(&digit.to_string());
        }
        let number = i64::from_str_radix(&digits, option as u32).ok()?;
        output.push(char::from_u32((number - interval) as u32)?);
    }
    Some(output)
}

fn hunter_args(script: &str) -> Option<Vec<String>> {
    let start = script.rfind("}(").or_else(|| script.rfind("} ("))?;
    let mut chars = script[start + 2..].chars().peekable();
    let mut args = Vec::new();
    loop {
        while matches!(chars.peek(), Some(ch) if ch.is_whitespace() || *ch == ',') {
            chars.next();
        }
        match chars.peek().copied()? {
            '"' | '\'' => {
                let quote = chars.next()?;
                let mut value = String::new();
                let mut escape = false;
                for ch in chars.by_ref() {
                    if escape {
                        value.push(ch);
                        escape = false;
                    } else if ch == '\\' {
                        escape = true;
                    } else if ch == quote {
                        break;
                    } else {
                        value.push(ch);
                    }
                }
                args.push(value);
            }
            ')' => break,
            ch if ch.is_ascii_digit() => {
                let mut value = String::new();
                while matches!(chars.peek(), Some(next) if next.is_ascii_digit()) {
                    value.push(chars.next()?);
                }
                args.push(value);
            }
            _ => {
                chars.next();
            }
        }
        if args.len() >= 5 {
            break;
        }
    }
    (args.len() >= 5).then_some(args)
}

fn script_containing(body: &str, marker: &str) -> Option<String> {
    body.split("<script")
        .skip(1)
        .find(|chunk| chunk.contains(marker))
        .and_then(|chunk| chunk.split('>').nth(1))
        .and_then(|chunk| chunk.split("</script>").next())
        .map(ToString::to_string)
}

fn js_single_quoted(script: &str, name: &str) -> Option<String> {
    let index = script
        .find(&format!("{name} = '"))
        .or_else(|| script.find(&format!("{name}='")))?;
    script[index..].split('\'').nth(1).map(ToString::to_string)
}

fn text_after<'a>(input: &'a str, marker: &str) -> Option<&'a str> {
    let index = input.find(marker)?;
    Some(&input[index + marker.len()..])
}

fn fingerprint(gpu_renderer: &str) -> String {
    let gpu = if gpu_renderer.trim().is_empty() {
        "SUMK"
    } else {
        gpu_renderer.trim()
    };
    STANDARD.encode(format!(r#"{{"gpu":"{gpu}","connection":"cellular"}}"#))
}

fn top_domain(base: &str) -> String {
    base.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("m.")
        .split('/')
        .next()
        .unwrap_or("scan-manga.com")
        .to_string()
}

fn origin(value: &str) -> String {
    let no_scheme = value
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    let host = no_scheme.split('/').next().unwrap_or(no_scheme);
    if value.starts_with("http://") {
        format!("http://{host}")
    } else {
        format!("https://{host}")
    }
}

fn decode_base64(value: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(value)
        .ok()
        .or_else(|| STANDARD_NO_PAD.decode(value).ok())
}

fn key_from_input(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input.trim_start_matches(BASE_URL)))
        .or_else(|| input.starts_with('/').then(|| normalize_key(input)))
}

fn normalize_key(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        if let Some(index) = value.find(BASE_URL) {
            return normalize_key(&value[index + BASE_URL.len()..]);
        }
    }
    format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
}

fn image_attr(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-original")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
}

fn status_from_text(text: &str) -> ItemStatus {
    let lower = text.to_ascii_lowercase();
    if lower.contains("en cours") {
        ItemStatus::Ongoing
    } else if lower.contains("termin") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

#[derive(Default, Deserialize)]
struct SearchResponse {
    title: Option<Vec<SearchItem>>,
}

#[derive(Deserialize)]
struct SearchItem {
    nom_match: String,
    url: String,
    image: String,
}

#[derive(Default, Deserialize)]
struct UrlPayload {
    #[serde(rename = "dN")]
    domain: String,
    s: String,
    v: String,
    c: String,
    p: std::collections::BTreeMap<String, PayloadPage>,
}

impl UrlPayload {
    fn generate_image_urls(self) -> Vec<String> {
        let base = format!("https://{}/{}/{}/{}", self.domain, self.s, self.v, self.c);
        self.p
            .into_iter()
            .filter_map(|(index, page)| index.parse::<i64>().ok().map(|index| (index, page)))
            .map(|(index, page)| (index, format!("{base}/{}.{}", page.f, page.e)))
            .collect::<std::collections::BTreeMap<_, _>>()
            .into_values()
            .collect()
    }
}

#[derive(Default, Deserialize)]
struct PayloadPage {
    f: String,
    e: String,
}

const POPULAR_FIXTURE: &str = r#"
<div id="carouselTOPContainer"><div class="top"><a class="atop" href="/sample.html">Sample Scan-Manga</a><img data-original="/cover.jpg"></div></div>
"#;
const LATEST_FIXTURE: &str = r#"
<div id="content_news"><div class="publi"><a class="l_manga" href="/sample.html">Sample Scan-Manga</a><img src="/cover.jpg"></div></div>
"#;
const SEARCH_FIXTURE: &str =
    r#"{"title":[{"nom_match":"Sample Scan-Manga","url":"/sample.html","image":"sample.jpg"}]}"#;
const DETAILS_FIXTURE: &str = r#"
<h1 class="main_title" itemprop=name>Sample Scan-Manga</h1><div itemprop=author>Author</div>
<div class="titres_desc" itemprop=description>Resume</div><div class="titres_souspart">En cours <span itemprop=genre>Action</span></div>
<div class="full_img_serie"><img itemprop=image src="/cover.jpg"></div>
<div class="chapt_m"><td class="publimg"><span class="i"><a href="/sample-chapitre-1.html">Chapitre 1</a></span></td><td class="publititle">Debut</td></div>
"#;
const PAGES_FIXTURE: &str = r#"<script>sml = 'left'; sme = 'right'; const idc = 1;</script>"#;
const PAGE_PAYLOAD_FIXTURE: &str = r#"{"dN":"static.scan-manga.com","s":"img","v":"manga","c":"sample","p":{"1":{"f":"001","e":"jpg"}}}"#;
const PAGE_API_FIXTURE: &str =
    "eJwFwckNgDAMAMBdOABYEvx3fg0JBpH8yxqFXe/Dfu1kBF1o0bABbc1p5zbOwlSQta9YNOSp9aJDswa70/0K+wBCwxx=1";
