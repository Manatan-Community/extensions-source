use manatan_extension::{
    abi::ExtensionResult, export_novel_source, source::NovelSource, CatalogItem, HomeSection,
    HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage, NovelText, Paged,
    UrlResolveResult,
};
use manatan_shared::{dates, html, lnreader, novel, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: RoyalRoad = RoyalRoad;
const BASE_URL: &str = "https://www.royalroad.com";

struct RoyalRoad;

impl NovelSource for RoyalRoad {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let latest = request
            .get("listing")
            .or_else(|| request.get("listingId"))
            .and_then(Value::as_str)
            .is_some_and(|listing| listing == "latest");
        let target = search_url(&request, "", page, latest);
        let body = fetch(&target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: lnreader::has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(key) = lnreader::key_from_url(BASE_URL, query) {
            return Ok(Paged {
                entries: vec![fetch_details(&key, false)],
                has_next_page: false,
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch(&search_url(&request, query, page, false), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body),
            has_next_page: lnreader::has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "fiction/1/sample".to_string());
        let enable_volume = lnreader::preference_bool(&request, "enableVol", false);
        Ok(fetch_details(&key, enable_volume))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let key =
            novel::request_key(&request, "novel").unwrap_or_else(|| "fiction/1/sample".to_string());
        let enable_volume = lnreader::preference_bool(&request, "enableVol", false);
        let body = fetch(&absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_chapters(&body, enable_volume))
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
            .unwrap_or_else(|| "fiction/1/sample/chapter/1/chapter-1".to_string());
        let body = fetch(&absolute_url(&key), TEXT_FIXTURE);
        Ok(parse_text(&body, &key))
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let list = self.list(request)?;
        Ok(vec![HomeSection {
            id: "search".to_string(),
            title: "Fictions".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: list.entries,
            has_more: list.has_next_page,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = lnreader::key_from_url(BASE_URL, input) {
            return Ok(Some(UrlResolveResult {
                item: Some(fetch_details(&key, false)),
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

fn search_url(request: &Value, query: &str, page: u64, latest: bool) -> String {
    let mut params = vec![("page".to_string(), page.to_string())];
    if !query.is_empty() {
        params.push(("title".to_string(), query.to_string()));
        params.push(("globalFilters".to_string(), "true".to_string()));
    }
    if latest {
        params.push(("orderBy".to_string(), "last_update".to_string()));
    }
    for key in [
        "keyword",
        "author",
        "minPages",
        "maxPages",
        "minRating",
        "maxRating",
        "status",
        "orderBy",
        "dir",
        "type",
    ] {
        if let Some(value) = lnreader::filter_string_opt(request, key) {
            if value != "ALL" && !value.is_empty() {
                params.push((key.to_string(), value));
            }
        }
    }
    for (key, include_id, exclude_id) in [
        ("genres", "genresInclude", "genresExclude"),
        ("tags", "tagsInclude", "tagsExclude"),
        (
            "content_warnings",
            "contentWarningsInclude",
            "contentWarningsExclude",
        ),
    ] {
        let (mut include, mut exclude) = lnreader::filter_include_exclude(request, key);
        include.extend(lnreader::filter_array(request, include_id));
        exclude.extend(lnreader::filter_array(request, exclude_id));
        for value in include {
            params.push(("tagsAdd".to_string(), value));
        }
        for value in exclude {
            params.push(("tagsRemove".to_string(), value));
        }
    }
    let query = params
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url::query_escape(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{BASE_URL}/fictions/search?{query}")
}

fn fetch(target: &str, fixture: &str) -> String {
    lnreader::fetch_document(BASE_URL, target, fixture)
}

fn parse_listing(body: &str) -> Vec<CatalogItem> {
    body.split("fiction-list-item")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let parts = lnreader::normalize_key(BASE_URL, &href)
                .split('/')
                .take(3)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let key = parts.join("/");
            let cover = html::attr_after(block, "<img", "src").map(|image| absolute_url(&image));
            let title = html::attr_after(block, "<img", "alt")
                .or_else(|| lnreader::text_between_tag(block, "h2"))
                .unwrap_or_else(|| {
                    url::slug_from_url(&key).unwrap_or_else(|| "Fiction".to_string())
                });
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                content_rating: Some("safe".to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .collect()
}

fn fetch_details(key: &str, _enable_volume: bool) -> CatalogItem {
    let body = fetch(&absolute_url(key), DETAILS_FIXTURE);
    CatalogItem {
        key: lnreader::normalize_key(BASE_URL, key),
        title: lnreader::text_between_tag(&body, "h1")
            .unwrap_or_else(|| url::slug_from_url(key).unwrap_or_else(|| "Fiction".to_string())),
        cover: html::attr_after(&body, "thumbnail", "src")
            .or_else(|| html::attr_after(&body, "<img", "src"))
            .map(|image| absolute_url(&image)),
        description: lnreader::html_after_marker(&body, "class=\"description\"", "</div>")
            .map(|value| html::strip_tags(&value).replace("\n\n\n", "\n\n")),
        authors: body
            .split("<a")
            .skip(1)
            .find(|chunk| {
                html::attr(chunk, "href").is_some_and(|href| href.starts_with("/profile/"))
            })
            .and_then(|chunk| html::text_between(chunk, ">", "</a>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .into_iter()
            .collect(),
        tags: parse_tags(&body),
        status: parse_status(&body),
        url: Some(absolute_url(key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, enable_volume: bool) -> Vec<NovelChapter> {
    let scripts = body
        .split("<script")
        .skip(1)
        .filter_map(|chunk| html::text_between(chunk, ">", "</script>"))
        .collect::<Vec<_>>()
        .join("\n");
    let chapters = lnreader::js_array_value(&scripts, "window.chapters")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let volumes = lnreader::js_array_value(&scripts, "window.volumes")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    chapters
        .into_iter()
        .filter_map(|chapter| {
            let raw = chapter.get("url")?.as_str()?;
            let parts = lnreader::normalize_key(BASE_URL, raw)
                .split('/')
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            let key = if parts.len() >= 5 {
                format!("{}/{}/{}/{}", parts[0], parts[1], parts[3], parts[4])
            } else {
                parts.join("/")
            };
            let volume = if enable_volume {
                chapter
                    .get("volumeId")
                    .and_then(Value::as_i64)
                    .and_then(|id| {
                        volumes
                            .iter()
                            .find(|volume| volume.get("id").and_then(Value::as_i64) == Some(id))
                    })
                    .and_then(|volume| volume.get("title").and_then(Value::as_str))
                    .map(ToString::to_string)
            } else {
                None
            };
            Some(NovelChapter {
                key: key.clone(),
                title: chapter
                    .get("title")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                chapter_number: chapter
                    .get("order")
                    .and_then(Value::as_f64)
                    .map(|value| value as f32),
                date_uploaded: chapter
                    .get("date")
                    .and_then(Value::as_str)
                    .and_then(dates::parse_fixture_date),
                section: volume,
                url: Some(absolute_url(&key)),
                language: Some("en".to_string()),
                ..NovelChapter::default()
            })
        })
        .collect()
}

fn parse_text(body: &str, key: &str) -> NovelText {
    let hidden = body
        .split("<style>")
        .nth(1)
        .and_then(|style| style.split("display: none").next())
        .and_then(|style| style.rsplit('.').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let before_notes = note_blocks(body, true, hidden.as_deref());
    let content =
        chapter_content(body, hidden.as_deref()).unwrap_or_else(|| TEXT_FIXTURE.to_string());
    let after_notes = note_blocks(body, false, hidden.as_deref());
    let html_body = [before_notes, content, after_notes]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n<hr class=\"notes-separator\">\n");
    let normalized = novel::normalize_reader_html(&html_body);
    NovelText {
        html: Some(normalized.clone()),
        text: Some(novel::cleanup_text(&normalized)),
        base_url: Some(absolute_url(key)),
        css: Some("body { line-height: 1.7; } img { max-width: 100%; height: auto; } .notes-separator { margin: 1.5rem 0; }".to_string()),
        image_headers: novel::image_headers(BASE_URL),
        ..NovelText::default()
    }
}

fn chapter_content(body: &str, hidden_class: Option<&str>) -> Option<String> {
    lnreader::html_after_marker(body, "chapter-content", "</div>")
        .map(|value| remove_hidden(&value, hidden_class))
}

fn note_blocks(body: &str, before: bool, hidden_class: Option<&str>) -> String {
    let mut notes = Vec::new();
    let mut seen_chapter = false;
    for block in body.split("author-note-portlet").skip(1) {
        let note = html::text_between(block, ">", "</div>")
            .map(|value| remove_hidden(&value, hidden_class))
            .unwrap_or_default();
        if !seen_chapter
            && body
                .split(block)
                .next()
                .unwrap_or_default()
                .contains("chapter-content")
        {
            seen_chapter = true;
        }
        if before != seen_chapter && !note.trim().is_empty() {
            let class = if before {
                "author-note-before"
            } else {
                "author-note-after"
            };
            notes.push(format!("<div class=\"{class}\">{note}</div>"));
        }
    }
    notes.join("")
}

fn remove_hidden(input: &str, hidden_class: Option<&str>) -> String {
    let Some(hidden_class) = hidden_class else {
        return input.to_string();
    };
    let mut output = input.to_string();
    while let Some(pos) = output.find(hidden_class) {
        let start = output[..pos].rfind('<').unwrap_or(pos);
        let end = output[pos..]
            .find("</div>")
            .map(|idx| pos + idx + 6)
            .unwrap_or(pos + hidden_class.len());
        output.replace_range(start..end.min(output.len()), "");
    }
    output
}

fn parse_tags(body: &str) -> Vec<String> {
    body.split("tags")
        .skip(1)
        .flat_map(|block| block.split("<a").skip(1))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn parse_status(body: &str) -> ItemStatus {
    let lower = body.to_ascii_lowercase();
    if lower.contains("completed") {
        ItemStatus::Completed
    } else if lower.contains("hiatus") {
        ItemStatus::Hiatus
    } else if lower.contains("dropped") {
        ItemStatus::Cancelled
    } else if lower.contains("ongoing") {
        ItemStatus::Ongoing
    } else {
        ItemStatus::Unknown
    }
}

fn absolute_url(input: &str) -> String {
    lnreader::absolute_url(BASE_URL, input)
}

const LIST_FIXTURE: &str = r#"
<div class="fiction-list-item"><figure><a href="/fiction/1/sample"><img src="/cover.jpg" alt="Sample Fiction"></a></figure></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<h1>Sample Fiction</h1><a href="/profile/1">Sample Author</a><img class="thumbnail" src="/cover.jpg"><div class="description"><p>Sample summary.</p></div><span class="tags"><a>Fantasy</a></span><span class="label-sm">ONGOING</span><script>window.chapters = [{"id":1,"volumeId":1,"title":"Chapter 1","date":"2024-01-01","order":1,"url":"/fiction/1/sample/chapter/1/chapter-1"}]; window.volumes = [{"id":1,"title":"Volume 1","cover":"","order":1}];</script>
"#;
const TEXT_FIXTURE: &str = r#"<div class="chapter-content"><p>Sample text.</p></div>"#;

export_novel_source!(SOURCE);
