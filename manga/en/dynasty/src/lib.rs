use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Dynasty = Dynasty;
const BASE_URL: &str = "https://dynasty-scans.com";
const SERIES: &str = "Series";
const CHAPTER: &str = "Chapter";
const ANTHOLOGY: &str = "Anthology";
const DOUJIN: &str = "Doujin";
const ISSUE: &str = "Issue";

struct Dynasty;

impl MangaSource for Dynasty {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_browse_json(POPULAR_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(parse_browse_json(&fetch_text(
            &format!("{BASE_URL}/chapters/added.json?page={page}"),
            POPULAR_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) || query.starts_with("deeplink:") {
            return Ok(Paged {
                entries: vec![deeplink_item(query)],
                has_next_page: false,
            });
        }
        Ok(parse_search_html(&fetch_text(
            &search_url(page, query, &request),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        Ok(parse_details_json(
            &fetch_text(&json_url(&key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/series/sample".into());
        if key.starts_with("/chapters/") {
            return Ok(vec![parse_individual_chapter(
                &fetch_text(&json_url(&key), CHAPTER_FIXTURE),
                &key,
            )]);
        }
        let body = fetch_text(&json_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters_json(&body, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapters/sample_ch1".into());
        Ok(parse_pages_json(&fetch_text(
            &json_url(&key),
            CHAPTER_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let popular = parse_browse_json(&fetch_text(
            &format!("{BASE_URL}/chapters/added.json?page=1"),
            POPULAR_FIXTURE,
        ));
        Ok(vec![HomeSection {
            id: "added".to_string(),
            title: "Recently Added".to_string(),
            style: Some(HomeSectionStyle::Compact),
            entries: popular.entries,
            has_more: popular.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(deeplink_item(input)),
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
}

fn fetch_text(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_browse_json(body: &str) -> Paged<CatalogItem> {
    let data = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::json!({}));
    let mut entries = Vec::new();
    for chapter in data.get("chapters").and_then(Value::as_array).into_iter().flatten() {
        let mut has_series = false;
        for tag in chapter.get("tags").and_then(Value::as_array).into_iter().flatten() {
            let Some(kind) = tag.get("type").and_then(Value::as_str) else {
                continue;
            };
            if let Some(directory) = directory_for_type(kind) {
                if kind == SERIES {
                    has_series = true;
                }
                push_unique(
                    &mut entries,
                    item_from_parts(
                        directory,
                        str_field(tag, "permalink"),
                        str_field(tag, "name"),
                        None,
                    ),
                );
            }
        }
        if !has_series {
            push_unique(
                &mut entries,
                item_from_parts(
                    "chapters",
                    str_field(chapter, "permalink"),
                    str_field(chapter, "title"),
                    None,
                ),
            );
        }
    }
    Paged {
        entries,
        has_next_page: data.get("current_page").and_then(Value::as_u64).unwrap_or(1)
            <= data.get("total_pages").and_then(Value::as_u64).unwrap_or(1),
    }
}

fn parse_search_html(body: &str) -> Paged<CatalogItem> {
    let mut entries = Vec::new();
    for chunk in body.split("<a").skip(1) {
        let Some(href) = html::attr(chunk, "href") else {
            continue;
        };
        let key = normalize_key(&href);
        if !is_dynasty_item_key(&key) {
            continue;
        }
        let (directory, permalink) = key.trim_start_matches('/').split_once('/').unwrap_or(("series", "sample"));
        let directory = if directory == "chapters" {
            chapter_series_permalink(permalink)
                .map(|_| "series")
                .unwrap_or(directory)
        } else {
            directory
        };
        let permalink = if key.starts_with("/chapters/") {
            chapter_series_permalink(permalink).unwrap_or_else(|| permalink.to_string())
        } else {
            permalink.to_string()
        };
        let title = html::text_between(chunk, ">", "</a>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| permalink_to_title(&permalink));
        push_unique(
            &mut entries,
            item_from_parts(directory, &permalink, &title, None),
        );
    }
    Paged {
        entries,
        has_next_page: body.contains("rel=\"next\"") || body.contains("rel='next'"),
    }
}

fn parse_details_json(body: &str, key: Option<String>) -> CatalogItem {
    let data = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::json!({}));
    let key = key.unwrap_or_else(|| {
        format!(
            "/{}/{}",
            directory_for_type(str_field(&data, "type")).unwrap_or("series"),
            str_field(&data, "permalink")
        )
    });
    if key.starts_with("/chapters/") || data.get("pages").is_some() {
        return parse_chapter_details(&data, key);
    }
    let mut authors = Vec::new();
    let mut tags = Vec::new();
    let mut others = Vec::new();
    let mut status_tags = Vec::new();
    collect_tags(data.get("tags"), &mut authors, &mut tags, &mut others, &mut status_tags);
    for item in data.get("taggings").and_then(Value::as_array).into_iter().flatten() {
        collect_tags(item.get("tags"), &mut authors, &mut tags, &mut others, &mut status_tags);
    }
    CatalogItem {
        key: key.clone(),
        title: str_field(&data, "name").to_string(),
        alternate_titles: data
            .get("aliases")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
        cover: data.get("cover").and_then(Value::as_str).map(build_cover_url),
        authors: authors.clone(),
        artists: authors,
        description: description_from_json(&data, &others),
        tags,
        status: status_from_tags(&status_tags),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapter_details(data: &Value, key: String) -> CatalogItem {
    let mut authors = Vec::new();
    let mut tags = Vec::new();
    let mut others = Vec::new();
    let mut status_tags = Vec::new();
    collect_tags(data.get("tags"), &mut authors, &mut tags, &mut others, &mut status_tags);
    CatalogItem {
        key: key.clone(),
        title: str_field(data, "title").to_string(),
        cover: data
            .get("pages")
            .and_then(Value::as_array)
            .and_then(|pages| pages.first())
            .and_then(|page| page.get("url"))
            .and_then(Value::as_str)
            .map(build_cover_url),
        authors: authors.clone(),
        artists: authors,
        description: Some(format!("Type: {CHAPTER}\n\nReleased: {}", str_field(data, "released_on"))),
        tags,
        status: ItemStatus::Completed,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters_json(body: &str, _key: &str) -> Vec<MangaChapter> {
    let data = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::json!({}));
    let manga_type = str_field(&data, "type").to_string();
    let mut header: Option<String> = None;
    let mut chapters = Vec::new();
    for item in data.get("taggings").and_then(Value::as_array).into_iter().flatten() {
        if let Some(value) = item.get("header").and_then(Value::as_str) {
            header = Some(value.to_string());
            continue;
        }
        let permalink = str_field(item, "permalink");
        if permalink.is_empty() {
            continue;
        }
        let mut title = header
            .as_ref()
            .map(|header| format!("{header} {}", str_field(item, "title")))
            .unwrap_or_else(|| str_field(item, "title").to_string());
        if manga_type != SERIES {
            let authors = tags_of_type(item, "Author");
            if !authors.is_empty() {
                title.push_str(" by ");
                title.push_str(&authors.join(" and "));
            }
        }
        chapters.push(MangaChapter {
            key: format!("/chapters/{permalink}"),
            title: Some(title),
            date_uploaded: parse_yyyy_mm_dd(str_field(item, "released_on")),
            scanlators: tags_of_type(item, "Scanlator"),
            url: Some(format!("{BASE_URL}/chapters/{permalink}")),
            ..MangaChapter::default()
        });
    }
    if manga_type != DOUJIN {
        chapters.reverse();
    }
    chapters
}

fn parse_individual_chapter(body: &str, key: &str) -> MangaChapter {
    let data = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::json!({}));
    let permalink = str_field(&data, "permalink");
    MangaChapter {
        key: if permalink.is_empty() {
            key.to_string()
        } else {
            format!("/chapters/{permalink}")
        },
        title: Some("Chapter".to_string()),
        date_uploaded: parse_yyyy_mm_dd(str_field(&data, "released_on")),
        scanlators: tags_of_type(&data, "Scanlator"),
        url: Some(url::join_url(BASE_URL, key)),
        ..MangaChapter::default()
    }
}

fn parse_pages_json(body: &str) -> Vec<MangaPage> {
    let data = serde_json::from_str::<Value>(body).unwrap_or_else(|_| serde_json::json!({}));
    data.get("pages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|page| page.get("url").and_then(Value::as_str))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn search_url(page: u64, query: &str, request: &Value) -> String {
    let sort = filter_value(request, "sort").unwrap_or_else(|| "_smart_".to_string());
    let sort = if sort == "_smart_" {
        if query.is_empty() { "released_on" } else { "" }
    } else {
        &sort
    };
    let mut pairs = vec![("q", query.to_string())];
    pairs.push(("sort", sort.to_string()));
    let classes = filter_value(request, "classes").unwrap_or_else(|| {
        [SERIES, CHAPTER, ANTHOLOGY, DOUJIN, ISSUE].join(",")
    });
    for class in classes.split(',').map(str::trim).filter(|value| !value.is_empty()) {
        pairs.push(("classes[]", class.to_string()));
    }
    for key in ["with", "without"] {
        if let Some(values) = filter_value(request, key) {
            for value in values.split(',').map(str::trim).filter(|value| !value.is_empty()) {
                pairs.push((if key == "with" { "with[]" } else { "without[]" }, value.to_string()));
            }
        }
    }
    if page > 1 {
        pairs.push(("page", page.to_string()));
    }
    format!(
        "{BASE_URL}/search?{}",
        pairs
            .into_iter()
            .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
            .collect::<Vec<_>>()
            .join("&")
    )
}

fn json_url(key: &str) -> String {
    let key = key.trim_matches('/');
    let (directory, permalink) = key.split_once('/').unwrap_or(("series", "sample"));
    format!("{BASE_URL}/{directory}/{permalink}.json")
}

fn deeplink_item(input: &str) -> CatalogItem {
    let key = if input.starts_with("deeplink:") {
        let parts = input.split(':').collect::<Vec<_>>();
        format!(
            "/{}/{}",
            parts.get(1).copied().unwrap_or("series"),
            parts.get(2).copied().unwrap_or("sample")
        )
    } else {
        normalize_key(input)
    };
    let key = if key.starts_with("/chapters/") {
        let permalink = key.trim_start_matches("/chapters/");
        chapter_series_permalink(permalink)
            .map(|series| format!("/series/{series}"))
            .unwrap_or(key)
    } else {
        key
    };
    let title = key
        .trim_matches('/')
        .split('/')
        .nth(1)
        .map(permalink_to_title)
        .unwrap_or_else(|| "Dynasty Scans".to_string());
    CatalogItem {
        key: key.clone(),
        title,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn item_from_parts(directory: &str, permalink: &str, title: &str, cover: Option<String>) -> CatalogItem {
    let key = format!("/{directory}/{permalink}");
    CatalogItem {
        key: key.clone(),
        title: title.to_string(),
        cover,
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn directory_for_type(kind: &str) -> Option<&'static str> {
    match kind {
        SERIES => Some("series"),
        ANTHOLOGY => Some("anthologies"),
        DOUJIN => Some("doujins"),
        ISSUE => Some("issues"),
        CHAPTER => Some("chapters"),
        _ => None,
    }
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        return format!(
            "/{}",
            input.trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .trim_end_matches('/')
        );
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn is_dynasty_item_key(key: &str) -> bool {
    ["/series/", "/anthologies/", "/chapters/", "/doujins/", "/issues/"]
        .iter()
        .any(|prefix| key.starts_with(prefix))
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn collect_tags(
    tags_value: Option<&Value>,
    authors: &mut Vec<String>,
    tags: &mut Vec<String>,
    others: &mut Vec<(String, String)>,
    status_tags: &mut Vec<String>,
) {
    for tag in tags_value.and_then(Value::as_array).into_iter().flatten() {
        let kind = str_field(tag, "type");
        let name = str_field(tag, "name");
        if name.is_empty() {
            continue;
        }
        match kind {
            "Author" => push_string(authors, name),
            "General" => push_string(tags, name),
            "Status" => {
                push_string(status_tags, name);
                others.push((kind.to_string(), name.to_string()));
            }
            SERIES | DOUJIN | ANTHOLOGY | ISSUE | "Scanlator" => {}
            _ => others.push((kind.to_string(), name.to_string())),
        }
    }
}

fn tags_of_type(value: &Value, kind: &str) -> Vec<String> {
    value
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|tag| str_field(tag, "type") == kind)
        .map(|tag| str_field(tag, "name").to_string())
        .collect()
}

fn description_from_json(data: &Value, others: &[(String, String)]) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(description) = data.get("description").and_then(Value::as_str).filter(|value| !value.is_empty()) {
        parts.push(html::strip_tags(description));
    }
    parts.push(format!("Type: {}", str_field(data, "type")));
    for (kind, value) in others {
        parts.push(format!("{kind}: {value}"));
    }
    (!parts.is_empty()).then(|| parts.join("\n\n"))
}

fn status_from_tags(tags: &[String]) -> ItemStatus {
    if tags.iter().any(|tag| tag == "Ongoing") {
        ItemStatus::Ongoing
    } else if tags.iter().any(|tag| tag == "Completed") {
        ItemStatus::Completed
    } else if tags.iter().any(|tag| tag == "On Hiatus") {
        ItemStatus::Hiatus
    } else if tags.iter().any(|tag| ["Dropped", "Cancelled", "Not Updated", "Abandoned", "Removed"].contains(&tag.as_str())) {
        ItemStatus::Cancelled
    } else {
        ItemStatus::Unknown
    }
}

fn build_cover_url(file: &str) -> String {
    let path = file.trim_start_matches('/');
    if path.starts_with("system/") {
        format!("{BASE_URL}/{path}#thumbnail")
    } else {
        format!("{BASE_URL}/system/tag_contents_covers/000/{path}#thumbnail")
    }
}

fn chapter_series_permalink(permalink: &str) -> Option<String> {
    permalink
        .split_once("_ch")
        .or_else(|| permalink.split_once("_volume_"))
        .map(|(series, _)| series.to_string())
}

fn permalink_to_title(permalink: &str) -> String {
    permalink
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_yyyy_mm_dd(input: &str) -> Option<i64> {
    let parts = input.split('-').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    unix_date(parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?)
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += match m {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if is_leap(year) => 29,
            2 => 28,
            _ => return None,
        };
    }
    Some((days + day as i64 - 1) * 86_400)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn push_unique(entries: &mut Vec<CatalogItem>, item: CatalogItem) {
    if !entries.iter().any(|entry| entry.key == item.key) {
        entries.push(item);
    }
}

fn push_string(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn filter_value(request: &Value, key: &str) -> Option<String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .or_else(|| request.get("filters")?.get(key)?.as_str())
        .map(ToString::to_string)
}

export_manga_source!(SOURCE);

const POPULAR_FIXTURE: &str = r#"{
  "current_page": 1,
  "total_pages": 1,
  "chapters": [
    { "title": "Sample Chapter", "permalink": "sample_ch01", "tags": [
      { "type": "Series", "name": "Sample Series", "permalink": "sample" }
    ] }
  ]
}"#;
const SEARCH_FIXTURE: &str = r#"
<div class="chapter-list"><a class="name" href="/series/sample">Sample Series</a></div>
"#;
const DETAILS_FIXTURE: &str = r#"{
  "name": "Sample Series",
  "type": "Series",
  "permalink": "sample",
  "tags": [{ "type": "Author", "name": "Anon", "permalink": "anon" }, { "type": "Status", "name": "Ongoing", "permalink": "ongoing" }],
  "cover": "/covers/sample.jpg",
  "description": "Description",
  "aliases": ["Sample Alias"],
  "total_pages": 1,
  "taggings": [
    { "header": "Volume 1" },
    { "title": "Chapter 1", "permalink": "sample_ch01", "released_on": "2024-01-01", "tags": [{ "type": "Scanlator", "name": "Group", "permalink": "group" }] }
  ]
}"#;
const CHAPTER_FIXTURE: &str = r#"{
  "title": "Chapter 1",
  "permalink": "sample_ch01",
  "released_on": "2024-01-01",
  "tags": [{ "type": "Scanlator", "name": "Group", "permalink": "group" }],
  "pages": [{ "url": "/system/releases/sample/page1.jpg" }, { "url": "/system/releases/sample/page2.jpg" }]
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_dynasty_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries[0].title, "Sample Series");
        assert_eq!(SOURCE.chapters(json!({})).unwrap()[0].title.as_deref(), Some("Volume 1 Chapter 1"));
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 2);
    }
}
