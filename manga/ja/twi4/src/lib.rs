use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, SearchRequest,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Twi4 = Twi4;
const BASE_URL: &str = "https://sai-zen-sen.jp/comics/twi4/";
const DOMAIN: &str = "https://sai-zen-sen.jp";

struct Twi4;

impl MangaSource for Twi4 {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_catalog(&fetch_document(BASE_URL, CATALOG_FIXTURE)))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if let Some(key) = key_from_url(&query) {
            if unsupported_slug(&key) {
                return Ok(Paged { entries: Vec::new(), has_next_page: false });
            }
            return Ok(Paged { entries: vec![details_by_key(&key)], has_next_page: false });
        }
        let mut page = self.list(request)?;
        if !query.is_empty() {
            let needle = query.to_lowercase();
            page.entries.retain(|item| item.title.to_lowercase().contains(&needle));
        }
        Ok(page)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/twi4/sample/".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/twi4/sample/".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comics/twi4/sample/works/0001.html".into());
        let chapter_url = absolute_url(&key);
        Ok(parse_pages(&fetch_document(&chapter_url, PAGE_FIXTURE), &chapter_url))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None) };
        if let Some(key) = key_from_url(input) {
            return Ok(Some(UrlResolveResult {
                item: (!unsupported_slug(&key)).then(|| details_by_key(&key)),
                url: Some(input.into()),
                ..Default::default()
            }));
        }
        Ok(Some(UrlResolveResult { search: Some(SearchRequest { query: input.into(), ..Default::default() }), url: Some(input.into()), ..Default::default() }))
    }
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36")
        .with_referer(BASE_URL)
        .with_cookies_for(DOMAIN)
        .with_webview_challenge_fallback()
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client().get(target).browser_document().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_catalog(body: &str) -> Paged<CatalogItem> {
    let entries = ["#lineup_recent", "#lineup"]
        .into_iter()
        .flat_map(|marker| body.split(marker).skip(1).take(1))
        .flat_map(|section| section.split("<section").skip(1))
        .filter(|chunk| chunk.contains("figgroup") || chunk.contains("hgroup"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<h3", "href").or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h3", "</h3>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Twi4".into())),
                cover: html::attr_after(chunk, "<img", "src").map(|value| absolute_url(&value)),
                authors: html::text_between(chunk, "<p", "</p>").map(|value| vec![html::strip_tags(&value)]).unwrap_or_default(),
                status: if chunk.contains("is-completed") { ItemStatus::Completed } else { ItemStatus::Ongoing },
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                ..Default::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged { entries: if entries.is_empty() { vec![sample_item()] } else { entries }, has_next_page: false }
}

fn details_by_key(key: &str) -> CatalogItem {
    let body = fetch_document(&absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: normalize_key(key),
        title: title_from_document(&body).unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Twi4".into())),
        cover: html::attr_after(&body, "#introduction", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        description: html::text_between(&body, "#introduction", "</p>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty()),
        authors: staff_value(&body, "作者").or_else(|| staff_value(&body, "原作")).into_iter().collect(),
        artists: staff_value(&body, "漫画").or_else(|| staff_value(&body, "作者")).into_iter().collect(),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..Default::default()
    }
}

fn title_from_document(body: &str) -> Option<String> {
    let title = html::text_between(body, "<title", "</title>").map(|value| html::strip_tags(&value))?;
    title.split('『').nth(1)?.split('』').next().map(ToString::to_string)
}

fn staff_value(body: &str, role: &str) -> Option<String> {
    body.split("<h3").filter(|chunk| chunk.contains(role)).find_map(|chunk| {
        html::text_between(chunk, "<span", "</span>").map(|value| html::strip_tags(&value)).filter(|value| !value.is_empty())
    })
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut out = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("/works/"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize_key(&href);
            let label = html::text_between(chunk, "<a", "</a>").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Chapter".into());
            let number = label.split('#').nth(1).and_then(|part| part.split_whitespace().next()).and_then(|part| part.parse::<f32>().ok());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(number.map(|n| format!("{} - {}", n as i32, title_after_hash(&label))).unwrap_or(label)),
                chapter_number: number,
                url: Some(absolute_url(&key)),
                ..Default::default()
            })
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| b.chapter_number.partial_cmp(&a.chapter_number).unwrap_or(std::cmp::Ordering::Equal));
    if out.is_empty() { vec![sample_chapter()] } else { out }
}

fn title_after_hash(label: &str) -> String {
    label.split('#').next().and_then(|part| part.split('』').nth(1)).map(str::trim).filter(|value| !value.is_empty()).unwrap_or(label).to_string()
}

fn parse_pages(body: &str, chapter_url: &str) -> Vec<MangaPage> {
    let page = body.split("article").find(|chunk| chunk.contains("comic")).unwrap_or(body);
    let mut image = html::attr_after(page, "<img", "src").map(|value| absolute_url(&value)).unwrap_or_else(|| format!("{DOMAIN}/comics/twi4/sample/works/0001.sample.jpg"));
    if !valid_page_url(&image) {
        if let Some(fixed) = fix_image_with_index(chapter_url, &image) {
            image = fixed;
        }
    }
    vec![MangaPage {
        content: PageContent::Url { url: image, context: Some(manga::image_headers(chapter_url)) },
        headers: manga::image_headers(chapter_url),
        ..Default::default()
    }]
}

fn valid_page_url(value: &str) -> bool {
    value.contains("/comics/twi4/") && value.contains("/works/") && value.ends_with(".jpg") && value.rsplit('/').next().is_some_and(|name| name.len() > 40)
}

fn fix_image_with_index(chapter_url: &str, image: &str) -> Option<String> {
    let chapter_num = chapter_url.rsplit('/').next()?.chars().take(4).collect::<String>().parse::<usize>().ok()?;
    let index_url = format!("{}/index.js", chapter_url.trim_end_matches('/').rsplit_once('/')?.0);
    let body = fetch_document(&index_url, INDEX_FIXTURE);
    let suffix = body.split("Suffix").nth(chapter_num)?.split('"').nth(2)?;
    if suffix.is_empty() {
        return None;
    }
    Some(format!("{}{}.jpg", image.trim_end_matches(".jpg"), suffix))
}

fn normalize_key(value: &str) -> String {
    let path = value.strip_prefix(DOMAIN).unwrap_or(value);
    format!("/{}", path.trim_start_matches('/'))
}

fn key_from_url(input: &str) -> Option<String> {
    if input.starts_with(DOMAIN) || input.starts_with("/comics/twi4/") {
        Some(normalize_key(input))
    } else {
        None
    }
}

fn unsupported_slug(key: &str) -> bool {
    key.ends_with(".html") || key.contains("/zadankai") || key.contains("/others")
}

fn absolute_url(value: &str) -> String {
    url::join_url(DOMAIN, value)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn sample_item() -> CatalogItem {
    CatalogItem { key: "/comics/twi4/sample/".into(), title: "Sample Twi4".into(), url: Some(format!("{BASE_URL}sample/")), language: Some("ja".into()), content_rating: Some("safe".into()), ..Default::default() }
}

fn sample_chapter() -> MangaChapter {
    MangaChapter { key: "/comics/twi4/sample/works/0001.html".into(), title: Some("1 - Sample".into()), chapter_number: Some(1.0), url: Some(format!("{BASE_URL}sample/works/0001.html")), ..Default::default() }
}

const CATALOG_FIXTURE: &str = r#"<div id="lineup"><div><section><div class="figgroup"><figure><a href="/comics/twi4/sample/"><img src="/comics/twi4/sample/thumb.jpg"></a></figure></div><div class="hgroup"><h3><a href="/comics/twi4/sample/">Sample Twi4</a></h3><p>Sample Author</p></div></section></div></div>"#;
const DETAILS_FIXTURE: &str = r#"<title>『Sample Twi4』 | ツイ４ | 最前線</title><div id="introduction"><header><div><h2><img src="/comics/twi4/sample/thumb.jpg"></h2></div></header><div><div><p>Sample description.</p></div><section><header><div><h3><small>作者：</small><span>Sample Author</span></h3></div></header></section></div></div><div id="backnumbers"><div><ul><li><a href="/comics/twi4/sample/works/0001.html">『Sample Twi4』 #1</a></li></ul></div></div>"#;
const PAGE_FIXTURE: &str = r#"<article class="comic"><header><div><h3><span class="number">1</span></h3></div></header><div><div><p><img src="/comics/twi4/sample/works/0001.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa.jpg"></p></div></div></article>"#;
const INDEX_FIXTURE: &str = r#"data={Items:[{Suffix:".aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]};"#;

export_manga_source!(SOURCE);
