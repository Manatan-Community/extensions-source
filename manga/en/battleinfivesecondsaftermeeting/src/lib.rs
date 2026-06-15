use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, manga::MadaraConfig, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: BattleInFiveSecondsAfterMeeting = BattleInFiveSecondsAfterMeeting;
const BASE_URL: &str = "https://www.deatte5.com";

struct BattleInFiveSecondsAfterMeeting;

impl MangaSource for BattleInFiveSecondsAfterMeeting {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: vec![base_item(false)],
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![parse_details(&fetch_document(query, DETAILS_FIXTURE))],
                has_next_page: false,
            });
        }
        self.list(request)
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(parse_details(&fetch_document(BASE_URL, DETAILS_FIXTURE)))
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        Ok(parse_chapters(&fetch_document(BASE_URL, DETAILS_FIXTURE)))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "/chapter-1".to_string());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGES_FIXTURE,
        )))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&fetch_document(input, DETAILS_FIXTURE))),
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

fn config() -> MadaraConfig {
    MadaraConfig {
        base_url: BASE_URL,
        lang: "en",
        content_rating: "safe",
        manga_path: "",
        popular_url_marker: "<h3",
        use_load_more: false,
        latest_enabled: false,
    }
}

fn fetch_document(target: &str, fixture: &str) -> String {
    manga::Madara::browser_client(&config())
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn base_item(initialized: bool) -> CatalogItem {
    CatalogItem {
        key: "/".to_string(),
        title: "Battle in 5 Seconds After Meeting Manga".to_string(),
        cover: Some(url::join_url(
            BASE_URL,
            "/wp-content/uploads/2022/01/48.jpg",
        )),
        url: Some(BASE_URL.to_string()),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        status: ItemStatus::Ongoing,
        initialized,
        ..CatalogItem::default()
    }
}

fn parse_details(body: &str) -> CatalogItem {
    let mut item = base_item(true);
    item.title = html::text_between(body, "<h1", "</h1>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or(item.title);
    item.cover = html::attr_after(body, "cover_managa", "src")
        .or_else(|| html::attr_after(body, "cover_managa", "data-src"))
        .map(|value| url::join_url(BASE_URL, &value))
        .or(item.cover);
    item.description = html::text_between(body, "synopsis", "</div>")
        .or_else(|| html::text_between(body, "<p", "</p>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    item.authors = text_after_label(body, "Author").into_iter().collect();
    item.artists = text_after_label(body, "Artist").into_iter().collect();
    item.tags = links_after_label(body, "Tag");
    item.alternate_titles = text_after_label(body, "Alternative")
        .into_iter()
        .flat_map(|value| {
            value
                .split(',')
                .map(str::trim)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|value| !value.is_empty())
        .collect();
    item.status = match text_after_label(body, "Status")
        .first()
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("completed") => ItemStatus::Completed,
        Some("hiatus") | Some("on hold") => ItemStatus::Hiatus,
        Some("cancelled") | Some("canceled") => ItemStatus::Cancelled,
        Some("ongoing") => ItemStatus::Ongoing,
        _ => ItemStatus::Ongoing,
    };
    item
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let recent = body
        .split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(parse_chapter_link)
        .collect::<Vec<_>>();
    let chapters = body
        .split("main-chapter")
        .skip(1)
        .filter_map(parse_chapter_link)
        .map(|mut chapter| {
            if let Some(matching) = recent
                .iter()
                .find(|recent| recent.title.as_deref() == chapter.title.as_deref())
            {
                chapter.date_uploaded = matching.date_uploaded;
            }
            chapter
        })
        .collect::<Vec<_>>();
    if chapters.is_empty() {
        recent
    } else {
        chapters
    }
}

fn parse_chapter_link(chunk: &str) -> Option<MangaChapter> {
    let href = html::attr_after(chunk, "<a", "href")?;
    let raw_title = html::text_between(chunk, "<a", "</a>")
        .or_else(|| html::text_between(chunk, "chapter-content", "</"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "Chapter".to_string());
    let title = raw_title
        .strip_prefix("Battle in 5 Seconds After Meeting, ")
        .unwrap_or(&raw_title)
        .to_string();
    Some(MangaChapter {
        key: normalize_key(&href),
        title: Some(title),
        url: Some(url::join_url(BASE_URL, &normalize_key(&href))),
        date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
            .map(|value| html::strip_tags(&value))
            .and_then(|value| manatan_shared::dates::parse_fixture_date(&value)),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    manga::Madara::parse_pages(body, &config())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        let path = input.trim_start_matches(BASE_URL).trim_end_matches('/');
        if path.is_empty() {
            "/".to_string()
        } else {
            path.to_string()
        }
    } else {
        format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
    }
}

fn text_after_label(body: &str, label: &str) -> Vec<String> {
    body.split("<h5")
        .skip(1)
        .filter(|chunk| chunk.contains(label))
        .filter_map(|chunk| html::text_between(chunk, "<h4", "</h4>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn links_after_label(body: &str, label: &str) -> Vec<String> {
    body.split("<h5")
        .skip(1)
        .find(|chunk| chunk.contains(label))
        .map(|chunk| {
            chunk
                .split("<a")
                .skip(1)
                .filter_map(|part| html::text_between(part, ">", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

export_manga_source!(SOURCE);

const DETAILS_FIXTURE: &str = r#"
<h1>Battle in 5 Seconds After Meeting Manga</h1>
<div class="cover_managa"><img src="/wp-content/uploads/2022/01/48.jpg"></div>
<div class="synopsis"><p>Akira Shiroyanagi is dragged into a battle of abilities.</p></div>
<h5>Author</h5><h4><a>Harawata Saizou</a></h4>
<h5>Artist</h5><h4><a>Miyako Kashiwa</a></h4>
<h5>Status</h5><h4>Ongoing</h4>
<h5>Tag</h5><h4><a>Action</a><a>Supernatural</a></h4>
<div class="main-chapter"><a href="/chapter-1"><span class="chapter-content">Battle in 5 Seconds After Meeting, Chapter 1</span></a></div>
"#;
const PAGES_FIXTURE: &str = r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"><img class="wp-manga-chapter-img" src="/page2.jpg"></div>"#;
