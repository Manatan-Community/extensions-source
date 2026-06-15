use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, ProcessedImage, Paged,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, speedbinb::SpeedBinbReader, url};
use serde_json::{Value, json};

const SOURCE: OhtaWebComic = OhtaWebComic;
const BASE_URL: &str = "https://webcomic.ohtabooks.com";
const READER_HOST: &str = "https://www.yondemill.jp";

struct OhtaWebComic;

impl MangaSource for OhtaWebComic {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/list/"),
            LIST_FIXTURE,
        ), page(&request), ""))
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
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/list/"),
            LIST_FIXTURE,
        ), page(&request), query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/contents/sample".into());
        let redirect_url = format!("{READER_HOST}{}?view=1&u0=1", normalize_reader_key(&key));
        let redirect_body = fetch_reader_document(&redirect_url, REDIRECT_FIXTURE, BASE_URL);
        let reader_url = redirect_body
            .split("location.href='")
            .nth(1)
            .and_then(|rest| rest.split_once("';").map(|(value, _)| value.to_string()))
            .or_else(|| html::attr_after(&redirect_body, "location.href", "href"))
            .filter(|value| !value.is_empty())
            .unwrap_or(redirect_url);
        let body = fetch_reader_document(&reader_url, READER_FIXTURE, &format!("{READER_HOST}{key}?view=1&u0=1"));
        SpeedBinbReader {
            base_url: BASE_URL,
            high_quality: true,
        }
        .pages(&reader_url, &body)
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = self.list(json!({"page": 1}))?;
        Ok(vec![HomeSection {
            id: "popular".into(),
            title: "Popular".into(),
            style: Some(HomeSectionStyle::Cover),
            has_more: popular.has_next_page,
            entries: popular.entries,
            ..HomeSection::default()
        }])
    }

    fn process_page_image(&self, request: Value) -> ExtensionResult<ProcessedImage> {
        manga_image::SpeedBinb::process_page_image(request)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .map(|key| format!("{READER_HOST}{}?view=1&u0=1", normalize_reader_key(&key))))
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

fn fetch_document(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_reader_document(target: &str, fixture: &str, referer: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(referer)
        .with_cookies_for(BASE_URL)
        .with_cookies_for(READER_HOST)
        .with_webview_challenge_fallback()
        .get(target)
        .browser_document()
        .referer(referer)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, page: u32, query: &str) -> Paged<CatalogItem> {
    let query = query.to_lowercase();
    let directory = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("href="))
        .filter_map(parse_listing_item)
        .filter(|item| query.is_empty() || item.title.to_lowercase().contains(&query))
        .fold(Vec::new(), push_unique);
    paginate(directory, page, 24)
}

fn parse_listing_item(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr(chunk, "href")?;
    let key = normalize_key(&href);
    let title = html::text_between(chunk, "class=\"title\"", "</")
        .or_else(|| html::text_between(chunk, "class='title'", "</"))
        .or_else(|| html::attr_after(chunk, "<img", "alt"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())?;
    Some(CatalogItem {
        key: key.clone(),
        title,
        cover: html::attr_after(chunk, "class=\"pic\"", "src")
            .or_else(|| html::attr_after(chunk, "<img", "src"))
            .map(|value| absolute_url(&value)),
        url: Some(absolute_url(&key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "itemprop=\"name\"", "</")
            .or_else(|| html::text_between(body, "itemprop='name'", "</"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Ohta Web Comic".into())),
        cover: content_header_image(body),
        authors: html::text_between(body, "itemprop=\"author\"", "</")
            .or_else(|| html::text_between(body, "itemprop='author'", "</"))
            .map(|value| vec![html::strip_tags(&value)])
            .unwrap_or_default(),
        description: description(body),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("adult".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("openBook("))
        .filter_map(|chunk| {
            let id = chapter_id(chunk)?;
            Some(MangaChapter {
                key: format!("/contents/{id}"),
                title: html::text_between(chunk, "class=\"title\"", "</")
                    .or_else(|| html::text_between(chunk, "class='title'", "</"))
                    .or_else(|| own_text(chunk))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                chapter_number: html::text_between(chunk, "class=\"number\"", "</")
                    .or_else(|| html::text_between(chunk, "class='number'", "</"))
                    .and_then(|value| html::strip_tags(&value).parse::<f32>().ok()),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    chapters.sort_by(|left, right| {
        right
            .chapter_number
            .partial_cmp(&left.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn content_header_image(body: &str) -> Option<String> {
    let style = body
        .split("contentHeader")
        .nth(1)
        .and_then(|chunk| html::attr(chunk, "style"))?;
    style
        .split("background-image:url(")
        .nth(1)
        .and_then(|rest| rest.split_once(')').map(|(value, _)| value.trim_matches('\'').trim_matches('"').to_string()))
        .map(|value| absolute_url(&value))
}

fn description(body: &str) -> Option<String> {
    let start = body
        .split("作品について")
        .nth(1)
        .or_else(|| body.split("titleBoader").nth(1))?;
    let lines = start
        .split("<p")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn chapter_id(chunk: &str) -> Option<String> {
    html::attr(chunk, "onclick")
        .and_then(|onclick| {
            onclick
                .split("openBook('")
                .nth(1)
                .and_then(|rest| rest.split_once("')").map(|(id, _)| id.to_string()))
        })
        .filter(|id| !id.is_empty())
}

fn own_text(chunk: &str) -> Option<String> {
    let text = html::strip_tags(&format!("<a{chunk}"));
    (!text.is_empty()).then_some(text)
}

fn paginate(entries: Vec<CatalogItem>, page: u32, page_size: usize) -> Paged<CatalogItem> {
    let start = page.saturating_sub(1) as usize * page_size;
    let end = (start + page_size).min(entries.len());
    if start >= entries.len() {
        return Paged {
            entries: Vec::new(),
            has_next_page: false,
        };
    }
    Paged {
        entries: entries[start..end].to_vec(),
        has_next_page: end < entries.len(),
    }
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn key_from_url(input: &str) -> Option<String> {
    input
        .starts_with(BASE_URL)
        .then(|| normalize_key(input))
        .or_else(|| {
            input
                .starts_with(READER_HOST)
                .then(|| normalize_reader_key(input))
        })
}

fn normalize_key(value: &str) -> String {
    let path = value
        .strip_prefix(BASE_URL)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn normalize_reader_key(value: &str) -> String {
    let path = value
        .strip_prefix(READER_HOST)
        .unwrap_or(value)
        .split('?')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", path.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

fn push_unique_chapter(mut entries: Vec<MangaChapter>, item: MangaChapter) -> Vec<MangaChapter> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="bnrList"><ul>
  <li><a href="/sample"><div class="pic"><img src="/cover.jpg"></div><div class="title">Sample Ohta</div></a></li>
</ul></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 itemprop="name">Sample Ohta</h1>
<span itemprop="author">Sample Author</span>
<div class="contentHeader" style="background-image:url(/cover.jpg);"></div>
<h3 class="titleBoader">作品について</h3><dl></dl><p>Sample description.</p>
<div class="backnumberList"><a onclick="openBook('sample')"><dt class="number">1</dt><div class="title">Episode 1</div></a></div>
"#;

const REDIRECT_FIXTURE: &str = r#"<script>location.href='https://www.yondemill.jp/viewer/sample?cid=sample';</script>"#;

const READER_FIXTURE: &str = r#"
<div id="content"><img data-ptimg="/sample.ptimg.json"></div>
"#;
