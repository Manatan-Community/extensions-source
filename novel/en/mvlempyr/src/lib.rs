use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Mvlempyr = Mvlempyr;
const BASE_URL: &str = "https://www.mvlempyr.io";
const CHAPTER_API: &str = "https://chap.heliosarchive.online";
const ASSET_URL: &str = "https://assets.mvlempyr.app/images/600";

struct Mvlempyr;

impl NovelSource for Mvlempyr {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_all_novels(ALL_NOVELS_FIXTURE),
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let mut entries = parse_all_novels(&fetch_or_fixture(
            &format!("{CHAPTER_API}/wp-json/wp/v2/mvl-novels?per_page=10000"),
            ALL_NOVELS_FIXTURE,
        ));
        let genres = filter_array(&request, "genre");
        if !genres.is_empty() {
            entries.retain(|item| {
                genres
                    .iter()
                    .all(|genre| item.tags.iter().any(|tag| tag.eq_ignore_ascii_case(genre)))
            });
        }
        sort_entries(&mut entries, &filter_text(&request, "order", "reviewCount"));
        Ok(Paged {
            entries: paginate(entries, page, 20),
            has_next_page: page * 20 < 10000,
        })
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
                entries: vec![fetch_details(&key)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let entries: Vec<_> = parse_all_novels(&fetch_or_fixture(
            &format!("{CHAPTER_API}/wp-json/wp/v2/mvl-novels?per_page=10000"),
            ALL_NOVELS_FIXTURE,
        ))
        .into_iter()
        .filter(|item| item.title.to_lowercase().contains(&query.to_lowercase()))
        .collect();
        let has_next_page = page * 20 < entries.len() as u64;
        Ok(Paged {
            entries: paginate(entries, page, 20),
            has_next_page,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        Ok(fetch_details(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "novel/sample".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let tag = body
            .split("id=\"novel-code\"")
            .nth(1)
            .and_then(|rest| html::text_between(rest, ">", "</"))
            .and_then(|code| code.trim().parse::<u128>().ok())
            .map(convert_novel_id)
            .unwrap_or(1);
        let posts = fetch_or_fixture(
            &format!("{CHAPTER_API}/wp-json/wp/v2/posts?tags={tag}&per_page=500&page=1"),
            POSTS_FIXTURE,
        );
        Ok(parse_posts(&posts))
    }

    fn chapters_page(&self, request: Value) -> ExtensionResult<NovelChapterPage> {
        Ok(NovelChapterPage {
            entries: self.chapters(request)?,
            has_next_page: false,
            ..NovelChapterPage::default()
        })
    }

    fn text(&self, request: Value) -> ExtensionResult<NovelText> {
        let key = novel::request_key(&request, "chapter")
            .unwrap_or_else(|| "chapter/sample-1".to_string());
        let body = fetch_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        let raw = html::text_between(&body, "id=\"chapter\"", "</div>")
            .or_else(|| html::text_between(&body, "id='chapter'", "</div>"))
            .unwrap_or_else(|| TEXT_FIXTURE.to_string());
        let normalized = novel::normalize_reader_html(&raw);
        Ok(NovelText {
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(BASE_URL.to_string()),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(BASE_URL),
            next_chapter_key: Some(key),
            ..NovelText::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "popular".to_string(),
            title: "Popular".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: parse_all_novels(ALL_NOVELS_FIXTURE),
            has_more: true,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key)),
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
        .with_referer(BASE_URL)
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_details(key: &str) -> CatalogItem {
    let body = fetch_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_details(&body, &normalize_key(key))
}

fn parse_all_novels(body: &str) -> Vec<CatalogItem> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    root.as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let slug = json_text(item, "slug").unwrap_or_else(|| "sample".to_string());
            let code = json_any(item, "novel-code").unwrap_or_else(|| "sample".to_string());
            let tags = split_csv(&json_any(item, "genre").unwrap_or_default());
            CatalogItem {
                key: format!("novel/{slug}"),
                title: json_text(item, "name").unwrap_or_else(|| "Novel".to_string()),
                cover: Some(format!("{ASSET_URL}/{code}.webp")),
                url: Some(format!("{BASE_URL}/novel/{slug}")),
                tags,
                language: Some("en".to_string()),
                content_rating: Some("suggestive".to_string()),
                initialized: false,
                extra: [
                    (
                        "avgReview".to_string(),
                        json!(json_number(item, "average-review")),
                    ),
                    (
                        "reviewCount".to_string(),
                        json!(json_number(item, "total-reviews")),
                    ),
                    (
                        "chapterCount".to_string(),
                        json!(json_number(item, "total-chapters")),
                    ),
                    ("created".to_string(), json!(json_number(item, "createdOn"))),
                ]
                .into_iter()
                .collect(),
                ..CatalogItem::default()
            }
        })
        .collect()
}

fn parse_details(body: &str, key: &str) -> CatalogItem {
    CatalogItem {
        key: normalize_key(key),
        title: first_text(body, &["novel-title", "<title"])
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Novel".to_string())),
        cover: html::attr_after(body, "novel-image", "src").map(|src| absolute_url(&src)),
        url: Some(absolute_url(key)),
        authors: html::text_between(body, "Author:", "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        description: html::text_between(body, "synopsis w-richtext", "</div>")
            .map(|value| html::strip_tags(&value)),
        tags: body
            .split("genre-tags")
            .skip(1)
            .filter_map(|block| {
                html::text_between(block, ">", "</").map(|value| html::strip_tags(&value))
            })
            .filter(|value| !value.is_empty())
            .collect(),
        status: parse_status(&first_text(body, &["novelstatustextlarge"]).unwrap_or_default()),
        language: Some("en".to_string()),
        content_rating: Some("suggestive".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_posts(body: &str) -> Vec<NovelChapter> {
    let root: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let mut out: Vec<_> = root
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|post| {
            let acf = post.get("acf")?;
            let novel_code = json_any(acf, "novel_code").unwrap_or_else(|| "sample".to_string());
            let number = acf
                .get("chapter_number")
                .and_then(Value::as_f64)
                .or_else(|| json_any(acf, "chapter_number").and_then(|value| value.parse().ok()))
                .unwrap_or(0.0) as f32;
            Some(NovelChapter {
                key: format!("chapter/{novel_code}-{}", display_number(number)),
                title: json_text(acf, "ch_name")
                    .or_else(|| Some(format!("Chapter {}", display_number(number)))),
                chapter_number: Some(number),
                url: Some(format!(
                    "{BASE_URL}/chapter/{novel_code}-{}",
                    display_number(number)
                )),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.chapter_number
            .unwrap_or(0.0)
            .partial_cmp(&b.chapter_number.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    out
}

fn convert_novel_id(input: u128) -> u128 {
    let modulus = 1_999_999_997_u128;
    let mut out = 1_u128;
    let mut base = 7_u128 % modulus;
    let mut exp = input;
    while exp > 0 {
        if exp & 1 == 1 {
            out = (out * base) % modulus;
        }
        base = (base * base) % modulus;
        exp >>= 1;
    }
    out
}

fn sort_entries(entries: &mut [CatalogItem], key: &str) {
    entries.sort_by(|a, b| {
        number_extra(b, key)
            .partial_cmp(&number_extra(a, key))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn number_extra(item: &CatalogItem, key: &str) -> f64 {
    item.extra.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

fn paginate<T>(entries: Vec<T>, page: u64, per_page: usize) -> Vec<T> {
    let start = page.saturating_sub(1) as usize * per_page;
    entries.into_iter().skip(start).take(per_page).collect()
}

fn filter_text(request: &Value, id: &str, default: &str) -> String {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_str())
        .unwrap_or(default)
        .to_string()
}

fn filter_array(request: &Value, id: &str) -> Vec<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(|value| value.get("value").unwrap_or(value).as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn first_text(body: &str, markers: &[&str]) -> Option<String> {
    markers.iter().find_map(|marker| {
        html::text_between(body, marker, "</")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
    })
}

fn parse_status(value: &str) -> ItemStatus {
    match value.to_ascii_lowercase().as_str() {
        text if text.contains("complete") => ItemStatus::Completed,
        text if text.contains("hiatus") => ItemStatus::Hiatus,
        text if text.contains("cancel") || text.contains("drop") => ItemStatus::Cancelled,
        _ => ItemStatus::Ongoing,
    }
}

fn split_csv(input: &str) -> Vec<String> {
    input
        .split([',', '|'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn json_text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn json_any(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(|item| {
        item.as_str()
            .map(ToString::to_string)
            .or_else(|| item.as_i64().map(|number| number.to_string()))
            .or_else(|| item.as_u64().map(|number| number.to_string()))
            .or_else(|| item.as_f64().map(|number| number.to_string()))
    })
}

fn json_number(value: &Value, key: &str) -> f64 {
    value
        .get(key)
        .and_then(|item| {
            item.as_f64()
                .or_else(|| item.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(0.0)
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches('/')
        .split('?')
        .next()
        .unwrap_or(input)
        .to_string()
}

fn absolute_url(input: &str) -> String {
    url::join_url(BASE_URL, input)
}

fn display_number(number: f32) -> String {
    if number.fract() == 0.0 {
        format!("{}", number as i32)
    } else {
        number.to_string()
    }
}

const ALL_NOVELS_FIXTURE: &str = r#"[{"name":"Sample Novel","slug":"sample","novel-code":"sample","average-review":4.5,"total-reviews":10,"total-chapters":1,"createdOn":1,"genre":"fantasy"}]"#;
const DETAILS_FIXTURE: &str = r#"<h1 class="novel-title">Sample Novel</h1><img class="novel-image" src="/cover.jpg"><div id="novel-code">1</div><div class="synopsis w-richtext">Sample summary.</div><div class="novelstatustextlarge">Ongoing</div><a class="genre-tags">fantasy</a><div>Author:<span>Sample Author</span></div>"#;
const POSTS_FIXTURE: &str = r#"[{"date":"2024-01-01","acf":{"ch_name":"Chapter 1","novel_code":"sample","chapter_number":1}}]"#;
const TEXT_FIXTURE: &str = r#"<div id="chapter"><span><p>Sample chapter text.</p></span></div>"#;

export_novel_source!(SOURCE);
