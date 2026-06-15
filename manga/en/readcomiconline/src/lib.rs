use base64::{Engine as _, engine::general_purpose};
use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

const SOURCE: ReadComicOnline = ReadComicOnline;
const BASE_URL: &str = "https://rcostation.xyz";
const MIRROR_URLS: [&str; 2] = ["https://readcomiconline.li", BASE_URL];

struct ReadComicOnline;

impl MangaSource for ReadComicOnline {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let path = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "LatestUpdate"
        } else {
            "MostPopular"
        };
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/ComicList/{path}?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = mirror_comic_key(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let target = if query.is_empty() {
            format!("{BASE_URL}/ComicList?page={page}")
        } else {
            format!(
                "{BASE_URL}/AdvanceSearch?comicName={}&page={page}",
                url::query_escape(query)
            )
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/Comic/sample".into());
        Ok(parse_details(
            &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/Comic/sample".into());
        Ok(parse_chapters(&fetch_document(
            &absolute_url(&key),
            DETAILS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/Comic/sample/Issue-1".into());
        let target = format!(
            "{}{}&s=&quality=hq&readType=1",
            absolute_url(&key),
            if key.contains('?') { "" } else { "?" }
        );
        let body = fetch_document(&target, PAGES_FIXTURE);
        let pages = decrypt_script_pages(&body, false);
        if pages.is_empty() {
            Ok(parse_img_pages(&body))
        } else {
            Ok(pages)
        }
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = mirror_comic_key(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch_document(&absolute_url(&key), DETAILS_FIXTURE),
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

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/Comic/"))
        .filter_map(|chunk| {
            let href = html::attr(chunk, "href")?;
            let key = normalize_key(&href);
            let title = html::text_between(chunk, ">", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: html::attr(chunk, "src").map(|image| absolute_url(&image)),
                url: Some(absolute_url(&key)),
                language: Some("en".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("Next") && body.contains("pager"),
    }
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| "/Comic/sample".into());
    let status_text = info_line(body, "Status:").unwrap_or_default();
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "bigChar", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Comic".into())),
        cover: html::attr_after(body, "rightBox", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: summary_text(body),
        authors: info_links(body, "Writer:"),
        artists: info_links(body, "Artist:"),
        tags: info_links(body, "Genres:"),
        status: parse_status(&status_text),
        url: Some(absolute_url(&key)),
        language: Some("en".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("<tr")
        .skip(2)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: html::text_between(chunk, "<a", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                url: Some(absolute_url(&key)),
                date_uploaded: html::text_between(chunk, "<td", "</td>")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| dates::parse_fixture_date(&value)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_img_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "data-src").or_else(|| html::attr(chunk, "src")))
        .enumerate()
        .map(|(index, image)| image_page(index, absolute_url(&image)))
        .collect()
}

fn decrypt_script_pages(body: &str, use_server2: bool) -> Vec<MangaPage> {
    let scripts = body
        .split("<script")
        .skip(1)
        .filter_map(|chunk| {
            let rest = chunk.split_once('>')?.1;
            Some(rest.split("</script>").next().unwrap_or("").trim())
        })
        .collect::<Vec<_>>()
        .join("\n");
    decrypt_links(&scripts, use_server2)
        .into_iter()
        .enumerate()
        .map(|(index, image)| image_page(index, image))
        .collect()
}

fn decrypt_links(script: &str, use_server2: bool) -> Vec<String> {
    let (pattern, replacement) = replacement_rule(script);
    let detected_base = base_url_re()
        .captures(script)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string());
    let mut raw_links = Vec::new();
    for var in array_re()
        .captures_iter(script)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
    {
        let call = Regex::new(&format!(
            r#"\w+\s*\([^)]*\b{}\b[^)]*,\s*["']([^"']{{20,}})["'][,\s]*\)"#,
            regex::escape(&var)
        ))
        .ok();
        let Some(call) = call else {
            continue;
        };
        let captures = call.captures_iter(script).collect::<Vec<_>>();
        let values = captures
            .iter()
            .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
            .collect::<Vec<_>>();
        let offset = prefix_offset(&values);
        for value in values {
            raw_links.push(decrypt_link(
                &value,
                offset,
                &pattern,
                replacement,
                detected_base.as_deref(),
                use_server2,
            ));
        }
    }
    let mut cleaned = Vec::new();
    for link in raw_links {
        let stable = link
            .split('?')
            .next()
            .unwrap_or("")
            .split('=')
            .next()
            .unwrap_or("");
        if stable.is_empty() || !stable.starts_with("http") || BLOCKLIST.contains(&stable) {
            continue;
        }
        if cleaned.iter().any(|existing: &String| {
            existing
                .split('?')
                .next()
                .unwrap_or("")
                .split('=')
                .next()
                .unwrap_or("")
                == stable
        }) {
            continue;
        }
        cleaned.push(link);
    }
    cleaned
}

fn replacement_rule(script: &str) -> (Regex, &str) {
    if let Some(cap) = replace_re().captures(script) {
        if let (Some(pattern), Some(replacement)) = (cap.get(1), cap.get(2)) {
            if let Ok(regex) = Regex::new(pattern.as_str()) {
                return (
                    regex,
                    Box::leak(replacement.as_str().to_string().into_boxed_str()),
                );
            }
        }
    }
    (
        Regex::new(r"\w{2}__\w{6}_").expect("valid fallback regex"),
        "e",
    )
}

fn decrypt_link(
    value: &str,
    offset: usize,
    pattern: &Regex,
    replacement: &str,
    detected_base: Option<&str>,
    use_server2: bool,
) -> String {
    let mut link = pattern
        .replace_all(value, replacement)
        .replace("pw_.g28x", "b")
        .replace("d2pr.x_27", "h");
    if offset != 0 && offset < link.len() {
        link = link[offset..].to_string();
    }
    if link.ends_with("=s0") || link.ends_with("=s1600") {
        link = link.replace("https://2.bp.blogspot.com/", "") + "?";
    }
    if link.starts_with("https") {
        return link;
    }
    let query = link
        .find('?')
        .map(|index| link[index..].to_string())
        .unwrap_or_default();
    let hires = link.contains("=s0?");
    let Some(size_index) = (if hires {
        link.find("=s0?")
    } else {
        link.find("=s1600?")
    }) else {
        return link;
    };
    let mut encoded = link[..size_index].to_string();
    if encoded.len() <= 50 {
        return link;
    }
    encoded = format!("{}{}", &encoded[15..33], &encoded[50..]);
    if encoded.len() < 11 {
        return link;
    }
    let len = encoded.len();
    encoded = format!(
        "{}{}{}",
        &encoded[..len - 11],
        &encoded[len - 2..len - 1],
        &encoded[len - 1..]
    );
    let Ok(decoded) = general_purpose::STANDARD.decode(encoded) else {
        return link;
    };
    let mut value = percent_decode(&String::from_utf8_lossy(&decoded));
    if value.len() > 19 {
        value = format!("{}{}", &value[..13], &value[17..]);
    }
    if value.len() > 2 {
        value = format!(
            "{}{}",
            &value[..value.len() - 2],
            if hires { "=s0" } else { "=s1600" }
        );
    }
    let base = detected_base.unwrap_or(if use_server2 {
        "https://ano1.rconet.biz/pic"
    } else {
        "https://2.bp.blogspot.com"
    });
    format!(
        "{base}/{value}{query}{}",
        if use_server2 { "&t=10" } else { "" }
    )
}

fn prefix_offset(values: &[String]) -> usize {
    let Some(first) = values.first() else {
        return 0;
    };
    let mut count = 0;
    for index in 0..first.len() {
        let Some(ch) = first.as_bytes().get(index) else {
            break;
        };
        if values
            .iter()
            .all(|value| value.as_bytes().get(index) == Some(ch))
        {
            count += 1;
            if count >= 5 && &first[count - 5..count] == "https" {
                return count - 5;
            }
        } else {
            break;
        }
    }
    0
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&input[index + 1..index + 3], 16) {
                output.push(hex);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn image_page(index: usize, image: String) -> MangaPage {
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: None,
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn info_line(body: &str, label: &str) -> Option<String> {
    body.split("<p")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(html::strip_tags)
        .filter(|value| !value.is_empty())
}

fn info_links(body: &str, label: &str) -> Vec<String> {
    body.split("<p")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .unwrap_or("")
        .split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn summary_text(body: &str) -> Option<String> {
    let summary = body
        .split("Summary:")
        .nth(1)?
        .split("</div>")
        .next()
        .unwrap_or("");
    let text = html::strip_tags(summary);
    (!text.is_empty()).then_some(text)
}

fn parse_status(input: &str) -> ItemStatus {
    if input.contains("Ongoing") {
        ItemStatus::Ongoing
    } else if input.contains("Completed") {
        ItemStatus::Completed
    } else {
        ItemStatus::Unknown
    }
}

fn mirror_comic_key(input: &str) -> Option<String> {
    MIRROR_URLS.iter().find_map(|mirror| {
        input.strip_prefix(mirror).and_then(|path| {
            let path = format!("/{}", path.trim_matches('/'));
            path.starts_with("/Comic/").then_some(path)
        })
    })
}

fn normalize_key(input: &str) -> String {
    mirror_comic_key(input).unwrap_or_else(|| format!("/{}", input.trim_matches('/')))
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn array_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"var\s+(\w+)\s*=\s*new\s+Array\(\)\s*;").expect("valid regex"))
}

fn replace_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"\.replace\(\s*/(\w+__\w+_)/g\s*,\s*['"](\w)['"]\s*\)"#).expect("valid regex")
    })
}

fn base_url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"baeu\(\w+,\s*["'](https?://[^"']+)["']\)"#).expect("valid regex")
    })
}

export_manga_source!(SOURCE);

const BLOCKLIST: &[&str] = &[
    "https://2.bp.blogspot.com/pw/AP1GczP6zCVVfdmN6OoVnm7CLvEfmHMUawyEwJWouX9C6SHwsiuYfLkUr9FsM6Zo34qNzPKeQeahBx9ckBZJQckiJmX1UwKD7uh900yz5rKyG4zT2rfIrqFviEJIev1Pg_pGRuSG57rIH6BDwGCTmiE4MjA",
    "https://2.bp.blogspot.com/pw/AP1GczP48thKMga7cud0tjtHtYqsvZzhYY0HyAxVzM3O1D6tkLbi0fT9NDZFFFH69hNnoGsnqJSEIh4mmpEoU1BJSfNXIz1f5aLXl41RM9os7ePn7ipbrYbIuqiQxAV0hhJZrNLl7FmauwLQ01paCrP6KAE",
    "https://2.bp.blogspot.com/pw/AP1GczNXprTMfAP2AHFFWvCbKq6qReXrqSohz87KeBjV0nh6XoLsE1NpzL7Rp9llxoY208IPARiIDON_TO6dZB0ZMNeB8J7xzUzbS9h6To7aGpOZshFofw-wFQ0KJ3y3wolSwzLrduZZ_0w8_6gGuTEB-98",
    "https://2.bp.blogspot.com/pw/AP1GczMVY_zWeag2n981CRX7jaZ73Sr0NtidtJhnvJ3-Rmh2fIo-PoQRI0ZksQEbpTjDHgBeNYbQ2hQodsY-Dv0FXUhiU_mus5z5L5lMVAH82kXYqOd2IEw",
    "https://2.bp.blogspot.com/pw/AP1GczOKY-6EDGVvlQGB2wj0xxB5JgcyiujFJC3CHgwqBOLIidwmoP6DLiMpX__Fw6MMPvLezN6soeV0A8pKSHUrC4rxZyO5vov40g1g4ipZdkFlzUouAFA",
    "https://2.bp.blogspot.com/pw/AP1GczO8AETT3k19nhJwxHm0sHCSy0tXyhSOYxnq3EUrmlvgY5yPqDaxcd1XZ7reQKH-lKgpGK4o3sW_9Yu6feqii79riXN3Ghi8Xs1S5Z4wi-aeHrq5PzOX",
];

const LIST_FIXTURE: &str = r#"<div class="list-comic"><div class="item"><a href="/Comic/sample"><img src="/cover.jpg">Sample Comic</a></div></div><ul class="pager"><li><a>Next</a></li></ul>"#;
const DETAILS_FIXTURE: &str = r#"<div class="rightBox"><img src="/cover.jpg"></div><div class="barContent"><a class="bigChar">Sample Comic</a><p><span>Writer:</span> <a>Writer</a></p><p><span>Artist:</span> <a>Artist</a></p><p><span>Genres:</span> <a>Action</a></p><p><span>Status:</span> Ongoing</p><p><span>Summary:</span></p><p>Summary text.</p><table class="listing"><tr></tr><tr></tr><tr><td><a href="/Comic/sample/Issue-1">Issue 1</a></td><td>01/01/2024</td></tr></table></div>"#;
const PAGES_FIXTURE: &str = r#"<img src="/page1.jpg"><img src="/page2.jpg">"#;
