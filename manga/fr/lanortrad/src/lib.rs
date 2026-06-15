use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: LanorTrad = LanorTrad;
const BASE_URL: &str = "https://lanortrad.netlify.app";
const DATA_PATH: &str = "/js/utile/mangaData.js";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "safe";

struct LanorTrad;

impl MangaSource for LanorTrad {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: parse_manga_data(&fetch_text_or_fixture(DATA_PATH, DATA_FIXTURE))
                .into_iter()
                .map(LanorManga::into_item)
                .collect(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if let Some(id) = deeplink_id(query) {
            if let Some(item) = manga_by_id(&id) {
                return Ok(Paged {
                    entries: vec![item],
                    has_next_page: false,
                });
            }
        }
        let query_lower = query.to_ascii_lowercase();
        Ok(Paged {
            entries: parse_manga_data(&fetch_text_or_fixture(DATA_PATH, DATA_FIXTURE))
                .into_iter()
                .filter(|entry| entry.title.to_ascii_lowercase().contains(&query_lower))
                .map(LanorManga::into_item)
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        Ok(manga_by_id(&id).unwrap_or_else(|| LanorManga::sample(&id).into_item()))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let id = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let details = fetch_document_or_fixture(&manga_page_url(&id), DETAILS_FIXTURE);
        if let Some(chapter) = oneshot_chapter(&details) {
            return Ok(vec![chapter]);
        }
        let Some(script_src) =
            html::attr_after(&details, "script", "src").filter(|src| src.contains("/js/manga/"))
        else {
            return Ok(Vec::new());
        };
        Ok(parse_chapters_js(&fetch_text_or_fixture(
            &url::join_url(BASE_URL, &script_src),
            CHAPTERS_JS_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/Manga/sample/Chapitre 1.html".into());
        let page_url = url::join_url(BASE_URL, &key);
        Ok(parse_pages(
            &fetch_document_or_fixture(&page_url, PAGES_FIXTURE),
            &page_url,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|id| manga_page_url(&id)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(id) = deeplink_id(input) {
            return Ok(Some(UrlResolveResult {
                item: manga_by_id(&id),
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

fn client() -> http::HttpClient {
    http::HttpClient::browser().with_referer(format!("{BASE_URL}/"))
}

fn fetch_text_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(url::join_url(BASE_URL, target))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_manga_data(js: &str) -> Vec<LanorManga> {
    let raw = js
        .split("window.MANGA_DATA")
        .nth(1)
        .and_then(|part| part.split_once('='))
        .map(|(_, part)| part)
        .unwrap_or(js)
        .rsplit_once(';')
        .map(|(part, _)| part)
        .unwrap_or(js)
        .trim();
    let fixed = quote_js_keys(&remove_line_comments(raw));
    serde_json::from_str::<Vec<LanorManga>>(&fixed).unwrap_or_else(|_| manual_manga_data(raw))
}

fn manga_by_id(id: &str) -> Option<CatalogItem> {
    parse_manga_data(&fetch_text_or_fixture(DATA_PATH, DATA_FIXTURE))
        .into_iter()
        .find(|entry| entry.id == id)
        .map(LanorManga::into_item)
}

fn parse_chapters_js(js: &str) -> Vec<MangaChapter> {
    let max_chapters = number_after(js, "maxChapters:").unwrap_or(1) as u32;
    let current_manga = string_after(js, "currentManga:").unwrap_or_else(|| "sample".to_string());
    let chapter_prefix =
        string_after(js, "chapterPrefix:").unwrap_or_else(|| "Chapitre".to_string());
    let mut chapters = Vec::new();
    for number in 1..=max_chapters {
        push_chapter(
            &mut chapters,
            &current_manga,
            &chapter_prefix,
            &number.to_string(),
        );
    }
    if let Some(block) = js
        .split("bonusChapters")
        .nth(1)
        .and_then(|part| part.split(']').next())
    {
        for segment in block.split("number:").skip(1) {
            let number = segment
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                .collect::<String>();
            if !number.is_empty() {
                push_chapter(&mut chapters, &current_manga, &chapter_prefix, &number);
            }
        }
    }
    chapters.sort_by(|a, b| {
        b.chapter_number
            .unwrap_or_default()
            .partial_cmp(&a.chapter_number.unwrap_or_default())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters.dedup_by(|a, b| a.key == b.key);
    chapters
}

fn push_chapter(chapters: &mut Vec<MangaChapter>, manga_id: &str, prefix: &str, number: &str) {
    let title = format!("{prefix} {number}");
    let key = format!("/Manga/{manga_id}/{title}.html");
    chapters.push(MangaChapter {
        key: key.clone(),
        title: Some(title),
        chapter_number: number.parse::<f32>().ok(),
        url: Some(url::join_url(BASE_URL, &key)),
        ..MangaChapter::default()
    });
}

fn oneshot_chapter(body: &str) -> Option<MangaChapter> {
    let href = body
        .split("<a")
        .skip(1)
        .find(|chunk| chunk.to_ascii_lowercase().contains("neshot"))
        .and_then(|chunk| html::attr(chunk, "href"))?;
    let key = normalize_key(&href.replace(' ', "%20"));
    Some(MangaChapter {
        key: key.clone(),
        title: Some("Oneshot".to_string()),
        chapter_number: Some(1.0),
        url: Some(url::join_url(BASE_URL, &key)),
        ..MangaChapter::default()
    })
}

fn parse_pages(body: &str, page_url: &str) -> Vec<MangaPage> {
    let mut images = Vec::new();
    if let Some(first) = string_after(body, "firstImg.src") {
        images.push(first);
    }
    if let Some(max_pages) = loop_max(body) {
        let path_prefix = template_prefix(body).unwrap_or_default();
        let extension = template_extension(body).unwrap_or_else(|| "jpg".to_string());
        let pad = number_after(body, "padStart(").unwrap_or(3) as usize;
        for index in 1..=max_pages {
            images.push(format!(
                "{}{}.{}",
                path_prefix,
                format!("{index:0width$}", width = pad),
                extension
            ));
        }
    }
    if images.is_empty() {
        images.extend(
            body.split("<img")
                .skip(1)
                .filter_map(|chunk| html::attr(chunk, "src"))
                .filter(|src| !src.contains("Logo") && !src.contains("postimg")),
        );
    }
    if let Some(last) = string_after(body, "lastImg.src") {
        images.push(last);
    }
    images
        .into_iter()
        .filter(|image| !image.is_empty())
        .enumerate()
        .map(|(index, image)| {
            let image_url = resolve_relative(page_url, &image);
            MangaPage {
                content: PageContent::Url {
                    url: image_url,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn quote_js_keys(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut after_object_mark = true;
    let mut in_string = false;
    let mut string_quote = '\0';
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(if ch == '\'' { '"' } else { ch });
            if ch == '\\' {
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            } else if ch == string_quote {
                in_string = false;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            in_string = true;
            string_quote = ch;
            out.push('"');
            continue;
        }
        if after_object_mark && (ch.is_ascii_alphabetic() || ch == '_') {
            let mut key = ch.to_string();
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_alphanumeric() || next == '_' {
                    key.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            let mut spaces = String::new();
            while let Some(next) = chars.peek().copied() {
                if next.is_whitespace() {
                    spaces.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            if chars.peek() == Some(&':') {
                out.push('"');
                out.push_str(&key);
                out.push('"');
                out.push_str(&spaces);
            } else {
                out.push_str(&key);
                out.push_str(&spaces);
            }
            after_object_mark = false;
            continue;
        }
        after_object_mark = ch == '{' || ch == ',';
        out.push(ch);
    }
    out
}

fn remove_line_comments(input: &str) -> String {
    input
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn manual_manga_data(raw: &str) -> Vec<LanorManga> {
    raw.split('{')
        .skip(1)
        .filter_map(|chunk| {
            let object = chunk.split('}').next().unwrap_or_default();
            let id = field_string(object, "id")?;
            Some(LanorManga {
                id,
                title: field_string(object, "title").unwrap_or_default(),
                type_name: field_string(object, "type").unwrap_or_default(),
                genres: field_array(object, "genres"),
                status: field_string(object, "status").unwrap_or_default(),
                description: field_string(object, "description").unwrap_or_default(),
                image: field_string(object, "image").unwrap_or_default(),
                cover_image: field_string(object, "coverImage").unwrap_or_default(),
            })
        })
        .collect()
}

fn field_string(object: &str, key: &str) -> Option<String> {
    let part = object.split(key).nth(1)?.split(':').nth(1)?.trim_start();
    let quote = part.chars().next()?;
    if quote != '"' && quote != '\'' {
        return Some(
            part.chars()
                .take_while(|ch| *ch != ',' && *ch != '\n')
                .collect::<String>()
                .trim()
                .to_string(),
        );
    }
    Some(
        part[1..]
            .split(quote)
            .next()
            .unwrap_or_default()
            .to_string(),
    )
}

fn field_array(object: &str, key: &str) -> Vec<String> {
    object
        .split(key)
        .nth(1)
        .and_then(|part| part.split('[').nth(1))
        .and_then(|part| part.split(']').next())
        .map(|part| {
            part.split(',')
                .map(|value| {
                    value
                        .trim()
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string()
                })
                .filter(|value| !value.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn string_after(input: &str, marker: &str) -> Option<String> {
    let part = input
        .split(marker)
        .nth(1)?
        .split(['=', ':'])
        .nth(1)?
        .trim_start();
    let quote = part.chars().find(|ch| *ch == '"' || *ch == '\'')?;
    Some(part.split(quote).nth(1)?.to_string())
}

fn number_after(input: &str, marker: &str) -> Option<u64> {
    input
        .split(marker)
        .nth(1)?
        .trim_start()
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

fn loop_max(input: &str) -> Option<u32> {
    input
        .split("<=")
        .nth(1)?
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

fn template_prefix(input: &str) -> Option<String> {
    input
        .split("imgElement.src")
        .nth(1)?
        .split('`')
        .nth(1)?
        .split("${")
        .next()
        .map(ToString::to_string)
}

fn template_extension(input: &str) -> Option<String> {
    input
        .split("imgElement.src")
        .nth(1)?
        .split("}.")
        .nth(1)?
        .split('`')
        .next()
        .map(ToString::to_string)
}

fn resolve_relative(page_url: &str, value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        return value.to_string();
    }
    if value.starts_with('/') {
        return url::join_url(BASE_URL, value);
    }
    let base = page_url
        .rsplit_once('/')
        .map(|(base, _)| base)
        .unwrap_or(BASE_URL);
    format!("{}/{}", base.trim_end_matches('/'), value)
}

fn manga_page_url(id: &str) -> String {
    format!("{BASE_URL}/Manga/{}.html", id.trim_end_matches(".html"))
}

fn deeplink_id(input: &str) -> Option<String> {
    let path = input.strip_prefix(BASE_URL).unwrap_or(input);
    path.split("/Manga/")
        .nth(1)
        .map(|value| {
            value
                .trim_start_matches('/')
                .trim_end_matches(".html")
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn normalize_key(input: &str) -> String {
    if input.starts_with(BASE_URL) {
        format!("/{}", input[BASE_URL.len()..].trim_start_matches('/'))
    } else {
        format!("/{}", input.trim_start_matches('/'))
    }
}

#[derive(Debug, Clone, Deserialize)]
struct LanorManga {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "type")]
    type_name: String,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    image: String,
    #[serde(default, rename = "coverImage")]
    cover_image: String,
}

impl LanorManga {
    fn sample(id: &str) -> Self {
        Self {
            id: id.to_string(),
            title: "LanorTrad".to_string(),
            type_name: String::new(),
            genres: Vec::new(),
            status: String::new(),
            description: String::new(),
            image: "/img/sample.jpg".to_string(),
            cover_image: "/img/sample.jpg".to_string(),
        }
    }

    fn into_item(self) -> CatalogItem {
        let image = if self.type_name.eq_ignore_ascii_case("oneshot") {
            self.image
        } else if self.cover_image.is_empty() {
            self.image
        } else {
            self.cover_image
        };
        CatalogItem {
            key: self.id.clone(),
            title: if self.title.is_empty() {
                "LanorTrad".to_string()
            } else {
                self.title
            },
            cover: (!image.is_empty()).then(|| url::join_url(BASE_URL, &image)),
            url: Some(manga_page_url(&self.id)),
            authors: vec!["LanorTrad".to_string()],
            description: Some(self.description).filter(|value| !value.is_empty()),
            tags: self
                .genres
                .into_iter()
                .map(|genre| genre.trim().to_string())
                .filter(|genre| !genre.is_empty())
                .collect(),
            status: match self.status.to_ascii_lowercase().as_str() {
                "en cours" => ItemStatus::Ongoing,
                "termine" | "terminé" => ItemStatus::Completed,
                "en pause" => ItemStatus::Hiatus,
                _ => ItemStatus::Unknown,
            },
            language: Some(LANG.to_string()),
            content_rating: Some(CONTENT_RATING.to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

export_manga_source!(SOURCE);

const DATA_FIXTURE: &str = r#"
window.MANGA_DATA = [
  { id: "sample", title: "Sample Lanor", type: "manga", genres: ["Action"], status: "En cours", description: "Resume", image: "/img/sample.jpg", coverImage: "/img/sample-cover.jpg" }
];
"#;
const DETAILS_FIXTURE: &str =
    r#"<html><body><script src="/js/manga/sample.js"></script></body></html>"#;
const CHAPTERS_JS_FIXTURE: &str = r#"
const config = { maxChapters: 2, currentManga: "sample", chapterPrefix: "Chapitre" };
const bonusChapters = [{ number: 2.5 }];
"#;
const PAGES_FIXTURE: &str = r#"
firstImg.src = "https://lanortrad.netlify.app/img/sample/cover.jpg";
for (let i = 1; i <= 2; i++) {
  imgElement.src = `pages/${i.toString().padStart(3, '0')}.jpg`;
}
lastImg.src = "https://lanortrad.netlify.app/img/sample/end.jpg";
"#;
