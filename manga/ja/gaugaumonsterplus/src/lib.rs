use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, Paged, ProcessedImage,
    SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    html, manga, manga_image, sdk::http::HttpClient, speedbinb::SpeedBinbReader, url,
};
use serde_json::{Value, json};

const SOURCE: GaugauMonsterPlus = GaugauMonsterPlus;
const BASE_URL: &str = "https://gaugau.futabanet.jp";

struct GaugauMonsterPlus;

impl MangaSource for GaugauMonsterPlus {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/list/works?page={page}"),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = key_from_url(query).filter(|key| is_manga_key(key)) {
            return Ok(Paged {
                entries: vec![details_by_key(&key)],
                has_next_page: false,
            });
        }

        let page = page(&request);
        let target = if !query.is_empty() {
            let mut target = format!(
                "{BASE_URL}/list/search-result?word={}",
                url::query_escape(query)
            );
            if page > 1 {
                target.push_str(&format!("&page={page}"));
            }
            target
        } else if let Some(genre) =
            filter_string(&request, "genre").filter(|value| !value.is_empty())
        {
            let mut target = format!("{BASE_URL}/list/tag/{}", url::query_escape(genre));
            if page > 1 {
                target.push_str(&format!("?page={page}"));
            }
            target
        } else if page > 1 {
            format!("{BASE_URL}/list/works?page={page}")
        } else {
            format!("{BASE_URL}/list/works")
        };
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/works/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/works/sample".into());
        Ok(parse_chapters(&fetch_document(
            &format!("{}/episodes", absolute_url(&key).trim_end_matches('/')),
            CHAPTERS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/episode/sample".into());
        let reader_url = absolute_url(&key);
        let body = fetch_document(&reader_url, READER_FIXTURE);
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
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(input).filter(|key| is_manga_key(key)) {
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("list__box")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<h4", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "<h4", "</h4>")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| {
                        url::slug_from_url(&key).unwrap_or_else(|| "がうがうモンスター＋".into())
                    }),
                cover: image_from_chunk(chunk),
                authors: author_values(chunk),
                tags: class_texts(chunk, "tag__item"),
                url: Some(absolute_url(&key)),
                language: Some("ja".into()),
                content_rating: Some("safe".into()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique);
    Paged {
        entries,
        has_next_page: body.contains("pagination")
            && body.contains("next")
            && !body.contains("next disabled"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    let key = normalize_key(key);
    parse_details(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE), &key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                url::slug_from_url(key).unwrap_or_else(|| "がうがうモンスター＋".into())
            }),
        cover: html::text_between(body, "list__box", "</div>")
            .and_then(|chunk| image_from_chunk(&chunk))
            .or_else(|| image_from_chunk(body)),
        authors: author_values(body),
        description: text_after_marker(body, "p class=\"mbOff")
            .or_else(|| text_after_marker(body, "p class='mbOff"))
            .or_else(|| {
                html::attr_after(body, "name=\"description\"", "content")
                    .map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty()),
        tags: class_texts(body, "tag__item"),
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("episode__grid")
        .skip(1)
        .filter(|chunk| {
            !chunk.contains("episode__button-app") && !chunk.contains("episode__button-complete")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let episode_num = class_texts(chunk, "episode__num")
                .into_iter()
                .next()
                .unwrap_or_else(|| "Episode".into());
            let episode_title = class_texts(chunk, "episode__title")
                .into_iter()
                .next()
                .filter(|title| !title.is_empty());
            let title = if let Some(episode_title) = episode_title {
                format!("{episode_num}「{episode_title}」")
            } else {
                episode_num.clone()
            };
            let key = normalize_key(&href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                chapter_number: chapter_number(&episode_num),
                url: Some(absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn author_values(body: &str) -> Vec<String> {
    block_after_marker(body, "list__text")
        .map(|block| link_texts(&block))
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| {
            link_texts(body)
                .into_iter()
                .filter(|value| !value.starts_with('#'))
                .collect()
        })
}

fn link_texts(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), push_unique_string)
}

fn class_texts(body: &str, class_name: &str) -> Vec<String> {
    body.split(class_name)
        .skip(1)
        .filter_map(text_from_open_tag_tail)
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .fold(Vec::new(), push_unique_string)
}

fn block_after_marker(body: &str, marker: &str) -> Option<String> {
    let rest = &body[body.find(marker)?..];
    let end = rest
        .find("</p>")
        .or_else(|| rest.find("</div>"))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn text_after_marker(body: &str, marker: &str) -> Option<String> {
    let start = body.find(marker)?;
    text_from_open_tag_tail(&body[start..]).map(|value| html::strip_tags(&value))
}

fn text_from_open_tag_tail(chunk: &str) -> Option<String> {
    let rest = chunk.split_once('>')?.1;
    let text = rest.split('<').next()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn image_from_chunk(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "img-books", "src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .map(|value| absolute_url(&value))
}

fn normalize_key(value: &str) -> String {
    let without_origin = value
        .strip_prefix(BASE_URL)
        .or_else(|| value.strip_prefix(BASE_URL.trim_start_matches("https://")))
        .unwrap_or(value)
        .split('#')
        .next()
        .unwrap_or(value)
        .trim_end_matches('/');
    format!("/{}", without_origin.trim_start_matches('/'))
}

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn key_from_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if input.starts_with('/') {
        return Some(normalize_key(input));
    }
    input.strip_prefix(BASE_URL).map(|path| normalize_key(path))
}

fn is_manga_key(key: &str) -> bool {
    key.contains("/works/")
}

fn page(request: &Value) -> u64 {
    request
        .get("page")
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .max(1)
}

fn filter_string<'a>(request: &'a Value, id: &str) -> Option<&'a str> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
}

fn chapter_number(value: &str) -> Option<f32> {
    let text = value.trim();
    let rest = text.strip_prefix('第')?;
    let (major, rest) = rest.split_once('話')?;
    let minor = rest
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or("0");
    Some(major.parse::<f32>().ok()? + minor.parse::<f32>().ok()? / 10.0)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

fn push_unique_string(mut items: Vec<String>, item: String) -> Vec<String> {
    if !items.iter().any(|existing| existing == &item) {
        items.push(item);
    }
    items
}

const LIST_FIXTURE: &str = r#"
<div class="works__grid">
  <div class="list__box">
    <a href="/works/sample"><img class="img-books" src="/sample.jpg" alt="Sample"></a>
    <h4><a href="/works/sample">Sample</a></h4>
    <p class="list__text"><span><a>Author</a></span></p>
    <a class="tag__item">異世界</a>
  </div>
</div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="mbOff"><h1>Sample</h1></div>
<div class="list__box"><div class="thumbnail"><img class="img-books" src="/sample.jpg"></div></div>
<p class="list__text"><span><a>Author</a></span></p>
<p class="mbOff">Summary</p>
<a class="tag__item">異世界</a>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<section id="episodes">
  <div class="episode__grid">
    <a href="/episode/sample"><span class="episode__num">第1話(0)</span><span class="episode__title">Sample</span></a>
  </div>
</section>
"#;

const READER_FIXTURE: &str = r#"<div id="content"></div>"#;

export_manga_source!(SOURCE);
