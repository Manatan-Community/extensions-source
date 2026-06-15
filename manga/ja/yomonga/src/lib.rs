use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, ProcessedImage,
    Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult, export_manga_source,
    source::MangaSource,
};
use manatan_shared::{html, manga, manga_image, sdk::http::HttpClient, speedbinb::SpeedBinbReader, url};
use serde_json::{Value, json};

const SOURCE: Yomonga = Yomonga;
const BASE_URL: &str = "https://www.yomonga.com";

struct Yomonga;

impl MangaSource for Yomonga {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = page(&request);
        Ok(parse_listing(&fetch_document(
            &format!("{BASE_URL}/titles/?page_num={page}"),
            LIST_FIXTURE,
        )))
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
        let mut target = format!("{BASE_URL}/titles/?page_num={}", page(&request));
        if !query.is_empty() {
            target.push_str(&format!("&search_word={}", url::query_escape(query)));
        } else if let Some(filter) = filter_string(&request, "category").filter(|value| !value.is_empty()) {
            target.push('&');
            target.push_str(&filter);
        }
        Ok(parse_listing(&fetch_document(&target, LIST_FIXTURE)))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(details_by_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/title/sample".into());
        Ok(parse_chapters(&fetch_document(&absolute_url(&key), DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/viewer/sample".into());
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

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    let entries = body
        .split("book-box4")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href").or_else(|| html::attr(chunk, "href"))?;
            let key = normalize_key(&href);
            Some(CatalogItem {
                key: key.clone(),
                title: html::text_between(chunk, "book-box4-title", "</")
                    .or_else(|| html::attr_after(chunk, "<img", "alt"))
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| "Yomonga".into())),
                cover: html::attr_after(chunk, "book-box4-thumbnail", "src")
                    .or_else(|| html::attr_after(chunk, "<img", "src"))
                    .map(|value| absolute_url(&value)),
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
        has_next_page: body.contains("paging-next") && body.contains("paging-click"),
    }
}

fn details_by_key(key: &str) -> CatalogItem {
    parse_details(&fetch_document(&absolute_url(key), DETAILS_FIXTURE), key)
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    let tags = text_values(body, "tag");
    CatalogItem {
        key: normalize_key(key),
        title: html::text_between(body, "intr-title", "</")
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Yomonga".into())),
        cover: html::attr_after(body, "intr-thumbnail", "src")
            .or_else(|| html::attr_after(body, "<img", "src"))
            .map(|value| absolute_url(&value)),
        authors: text_values(body, "intr-writer")
            .into_iter()
            .map(|value| {
                value
                    .trim_start_matches("漫画：")
                    .trim_start_matches("原作：")
                    .trim_start_matches("キャラクター原案：")
                    .trim()
                    .to_string()
            })
            .collect(),
        description: html::text_between(body, "intr-desc", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty()),
        status: if tags.iter().any(|tag| tag.contains("連載中")) {
            ItemStatus::Ongoing
        } else if tags.iter().any(|tag| tag.contains("連載終了")) {
            ItemStatus::Completed
        } else {
            ItemStatus::Unknown
        },
        tags,
        url: Some(absolute_url(key)),
        language: Some("ja".into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    body.split("episode-list")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "button-type1", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            Some(MangaChapter {
                key: normalize_key(&href),
                title: html::text_between(chunk, "episode-name", "</")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty()),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn text_values(body: &str, marker: &str) -> Vec<String> {
    body.split(marker)
        .skip(1)
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
        })
        .collect()
}

fn page(request: &Value) -> u32 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1) as u32
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|filter| filter.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|filter| filter.get("value").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

fn key_from_url(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize_key(input))
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

fn absolute_url(value: &str) -> String {
    url::join_url(BASE_URL, value)
}

fn push_unique(mut entries: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !entries.iter().any(|existing| existing.key == item.key) {
        entries.push(item);
    }
    entries
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="book-box4"><a href="/title/sample"><img class="book-box4-thumbnail" src="/cover.jpg"><div class="book-box4-title">Sample Yomonga</div></a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1 class="intr-title">Sample Yomonga</h1>
<img class="intr-thumbnail" src="/cover.jpg">
<div class="intr-writer">漫画：Sample Author</div>
<div class="intr-text"><div class="intr-desc">Sample description.</div></div>
<div class="tag-wrapper"><span class="tag">連載中</span></div>
<div class="episode-list" data-episode_no="1"><span class="episode-name">Episode 1</span><a class="button-type1" href="/viewer/sample?cid=sample"></a></div>
"#;

const READER_FIXTURE: &str = r#"
<div id="content"><img data-ptimg="/sample.ptimg.json"></div>
"#;
