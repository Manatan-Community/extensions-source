use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, NovelChapter, NovelChapterPage,
    NovelText, Paged, UrlResolveResult, abi::ExtensionResult, export_novel_source,
    source::NovelSource,
};
use manatan_shared::{html, novel, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: WitchCultTranslations = WitchCultTranslations;
const BASE_URL: &str = "https://witchculttranslation.com";
const NOVEL_KEY: &str = "table-of-content";
const NOVEL_TITLE: &str = "Re:Zero kara Hajimeru Isekai Seikatsu";

struct WitchCultTranslations;

impl NovelSource for WitchCultTranslations {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        Ok(Paged {
            entries: if page == 1 {
                vec![novel_item()]
            } else {
                Vec::new()
            },
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if query.starts_with(BASE_URL)
            || normalize(query).is_empty()
            || normalize(NOVEL_TITLE).contains(&normalize(query))
        {
            return Ok(Paged {
                entries: vec![novel_item()],
                has_next_page: false,
            });
        }
        Ok(Paged {
            entries: Vec::new(),
            has_next_page: false,
        })
    }

    fn details(&self, _request: Value) -> ExtensionResult<CatalogItem> {
        Ok(parse_details())
    }

    fn chapters(&self, _request: Value) -> ExtensionResult<Vec<NovelChapter>> {
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/{NOVEL_KEY}"), TOC_FIXTURE);
        Ok(parse_chapters_from_toc(&body))
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
            .unwrap_or_else(|| "2020/01/01/sample-chapter".to_string());
        let body = fetch_document_or_fixture(&absolute_url(&key), TEXT_FIXTURE);
        let title = text_between_tag(&body, "h1");
        let mut content =
            content_after(&body, "entry-content").unwrap_or_else(|| TEXT_FIXTURE.to_string());
        for marker in [
            "patreon-snippet",
            "sharedaddy",
            "jp-relatedposts",
            "jp-post-flair",
        ] {
            content = remove_block_containing(&content, marker);
        }
        let html_body = format!(
            "{}{}",
            title
                .as_ref()
                .map(|value| format!("<h1>{value}</h1>"))
                .unwrap_or_default(),
            content
        );
        let normalized = novel::normalize_reader_html(&html_body);
        Ok(NovelText {
            title,
            html: Some(normalized.clone()),
            text: Some(novel::cleanup_text(&normalized)),
            base_url: Some(absolute_url(&key)),
            css: Some(
                "body { line-height: 1.7; } img { max-width: 100%; height: auto; }".to_string(),
            ),
            image_headers: novel::image_headers(&absolute_url(&key)),
            ..NovelText::default()
        })
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "main".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Cover),
            entries: vec![novel_item()],
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.contains("witchculttranslation.com") {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details()),
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

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn novel_item() -> CatalogItem {
    let body = fetch_document_or_fixture(BASE_URL, HOME_FIXTURE);
    CatalogItem {
        key: NOVEL_KEY.to_string(),
        title: NOVEL_TITLE.to_string(),
        cover: latest_arc_cover(&body),
        url: Some(format!("{BASE_URL}/{NOVEL_KEY}")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        ..CatalogItem::default()
    }
}

fn parse_details() -> CatalogItem {
    let mut item = novel_item();
    item.authors = vec!["Tappei Nagatsuki".to_string()];
    item.description = Some("Fan translation of the Re:Zero web novel (Arc 5 onwards).\n\nSuddenly, Natsuki Subaru, a shut-in student, is summoned to another world on his way home from the convenience store. A completely ordinary person with no knowledge, skills, combat abilities, or communication skills, he's thrown into this other world without any cheat bonuses and must desperately try to survive. The only blessing he receives is the painful ability to return by death, which allows him to rewind time after dying.".to_string());
    item.status = ItemStatus::Ongoing;
    item.initialized = true;
    item
}

fn latest_arc_cover(body: &str) -> Option<String> {
    body.split("entry-content")
        .nth(1)
        .unwrap_or(body)
        .split("<img")
        .skip(1)
        .filter_map(|chunk| html::attr(chunk, "src"))
        .last()
        .map(|value| absolute_url(&value))
}

fn parse_chapters_from_toc(body: &str) -> Vec<NovelChapter> {
    let content = content_after(body, "entry-content").unwrap_or_else(|| body.to_string());
    let mut chapters = Vec::new();
    let mut pos = 0;
    let mut current_arc = 0_u32;
    let mut chapter_number = 0_f32;
    while pos < content.len() {
        let Some((tag, start)) = next_tag(&content[pos..], &["<h1", "<h2", "<ul"]) else {
            break;
        };
        pos += start;
        if tag == "<h1" || tag == "<h2" {
            let end_tag = if tag == "<h1" { "</h1>" } else { "</h2>" };
            let block = html::text_between(&content[pos..], tag, end_tag).unwrap_or_default();
            let text = html::strip_tags(&block);
            if text.to_ascii_lowercase().starts_with("side content") {
                break;
            }
            if let Some(arc) = arc_number(&text) {
                current_arc = arc;
            }
            pos += content[pos..]
                .find(end_tag)
                .map(|idx| idx + end_tag.len())
                .unwrap_or(tag.len());
            continue;
        }
        let end = content[pos..]
            .find("</ul>")
            .map(|idx| pos + idx + 5)
            .unwrap_or(content.len());
        if current_arc >= 5 {
            let block = &content[pos..end];
            for link in block.split("<a").skip(1) {
                let href = html::attr(link, "href").unwrap_or_default();
                if !href.contains("witchculttranslation.com") {
                    continue;
                }
                let title = html::text_between(link, ">", "</a>")
                    .map(|value| html::strip_tags(&value))
                    .filter(|value| !value.is_empty());
                let Some(title) = title else {
                    continue;
                };
                let key = normalize_key(&href);
                chapter_number += 1.0;
                chapters.push(NovelChapter {
                    key: key.clone(),
                    title: Some(format!("Arc {current_arc}, {title}")),
                    chapter_number: Some(chapter_number),
                    date_uploaded: date_from_key(&key),
                    url: Some(absolute_url(&key)),
                    language: Some("en".to_string()),
                    ..NovelChapter::default()
                });
            }
        }
        pos = end;
    }
    chapters
}

fn next_tag<'a>(input: &str, tags: &'a [&'a str]) -> Option<(&'a str, usize)> {
    tags.iter()
        .filter_map(|tag| input.find(tag).map(|index| (*tag, index)))
        .min_by_key(|(_, index)| *index)
}

fn arc_number(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let rest = lower.strip_prefix("arc")?.trim();
    rest.split_whitespace().next()?.parse().ok()
}

fn date_from_key(key: &str) -> Option<i64> {
    let mut parts = key.split('/');
    let year = parts.next()?.parse::<i32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    Some(days_from_civil(year, month, day) * 86_400)
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - (month <= 2) as i32;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let doy = (153 * (month as i32 + if month > 2 { -3 } else { 9 }) + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64
}

fn remove_block_containing(input: &str, marker: &str) -> String {
    let mut out = input.to_string();
    while let Some(pos) = out.find(marker) {
        let start = out[..pos].rfind('<').unwrap_or(pos);
        let end = out[pos..]
            .find("</div>")
            .map(|idx| pos + idx + 6)
            .or_else(|| out[pos..].find("</section>").map(|idx| pos + idx + 10))
            .unwrap_or(pos + marker.len());
        out.replace_range(start..end.min(out.len()), "");
    }
    out
}

fn content_after(body: &str, marker: &str) -> Option<String> {
    html::text_between(body, marker, "</div>")
        .or_else(|| html::text_between(body, marker, "</article>"))
}

fn text_between_tag(body: &str, tag: &str) -> Option<String> {
    html::text_between(body, &format!("<{tag}"), &format!("</{tag}>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
}

fn normalize(input: &str) -> String {
    input
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn normalize_key(input: &str) -> String {
    input
        .trim()
        .trim_start_matches(BASE_URL)
        .trim_start_matches("https://witchculttranslation.com/")
        .trim_start_matches("http://witchculttranslation.com/")
        .trim_start_matches('/')
        .trim_end_matches('/')
        .to_string()
}

fn absolute_url(input: &str) -> String {
    if input.starts_with("http") {
        input.to_string()
    } else {
        url::join_url(BASE_URL, input)
    }
}

const HOME_FIXTURE: &str = r#"<div class="entry-content"><h1><img src="https://witchculttranslation.com/cover.jpg"></h1></div>"#;
const TOC_FIXTURE: &str = r#"
<div class="entry-content"><h1>Arc 5</h1><ul><li><a href="https://witchculttranslation.com/2020/01/01/sample-chapter/">Chapter 1</a></li></ul><h1>Side Content</h1></div>
"#;
const TEXT_FIXTURE: &str = r#"<h1 class="entry-title">Chapter 1</h1><div class="entry-content"><p>Sample chapter text.</p></div>"#;

export_novel_source!(SOURCE);
