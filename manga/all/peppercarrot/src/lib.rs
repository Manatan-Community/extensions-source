use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: PepperCarrot = PepperCarrot;
const BASE_URL: &str = "https://www.peppercarrot.com";
const TITLE: &str = "Pepper&Carrot";
const AUTHOR: &str = "David Revoy";

const ARTWORK_KEYS: [&str; 11] = [
    "artworks",
    "wallpapers",
    "sketchbook",
    "misc",
    "book-publishing",
    "comissions",
    "eshop",
    "framasoft",
    "press",
    "references",
    "wiki",
];

struct PepperCarrot;

impl MangaSource for PepperCarrot {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: selected_entries(&request),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let mut entries = selected_entries(&request);
        if !query.is_empty() {
            entries.retain(|entry| entry.title.to_lowercase().contains(&query.to_lowercase()));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "en".into());
        Ok(item_for_key(&key).unwrap_or_else(|| comic_item(lang_data("en"))))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "en".into());
        let target = chapter_list_url(&key);
        let body = fetch_document_or_fixture(
            &target,
            if key.starts_with('#') {
                ARTWORK_FIXTURE
            } else {
                COMIC_FIXTURE
            },
        );
        Ok(if key.starts_with('#') {
            parse_artwork_chapters(&body, &target)
        } else {
            parse_comic_chapters(&body)
        })
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let high_res = request
            .get("preferences")
            .and_then(|prefs| prefs.get("highResolution"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/en/webcomics/ep001.html".into());
        if key.ends_with(".jpg") {
            return Ok(vec![image_page(
                0,
                &url::join_url(BASE_URL, &key),
                high_res,
            )]);
        }
        let target = url::join_url(BASE_URL, &key);
        let body = fetch_document_or_fixture(&target, PAGES_FIXTURE);
        Ok(parse_pages(&body, high_res))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = input
                .trim_start_matches(BASE_URL)
                .trim_start_matches('/')
                .to_string();
            let key = if key.is_empty() {
                "en".to_string()
            } else {
                format!("/{key}")
            };
            return Ok(Some(UrlResolveResult {
                item: item_for_key(key.trim_start_matches('/')),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
                ..SearchRequest::default()
            }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

fn selected_entries(request: &Value) -> Vec<CatalogItem> {
    let lang_keys = selected_languages(request);
    let mut entries = Vec::new();
    for key in lang_keys {
        let data = lang_data(&key);
        entries.push(comic_item(data.clone()));
        entries.push(mini_fantasy_item(data));
    }
    entries.extend(ARTWORK_KEYS.into_iter().map(artwork_item));
    entries
}

fn selected_languages(request: &Value) -> Vec<String> {
    let raw = request
        .get("preferences")
        .and_then(|prefs| prefs.get("languages"))
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .get("filters")
                .and_then(|filters| filters.get("languages"))
                .and_then(Value::as_str)
        })
        .unwrap_or("en");
    let mut keys = raw
        .split(',')
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if keys.is_empty() {
        keys.push("en".to_string());
    }
    keys
}

#[derive(Clone)]
struct LangData {
    key: String,
    name: String,
    progress: String,
    translators: String,
    title: String,
}

fn lang_data(key: &str) -> LangData {
    let episodes = fetch_json_or_fixture(
        &format!("{BASE_URL}/0_sources/episodes.json"),
        EPISODES_FIXTURE,
    );
    let langs = fetch_json_or_fixture(&format!("{BASE_URL}/0_sources/langs.json"), LANGS_FIXTURE);
    let total = episodes.as_array().map(Vec::len).unwrap_or(1).max(1);
    let translated = episodes
        .as_array()
        .into_iter()
        .flatten()
        .filter(|episode| {
            episode
                .get("translated_languages")
                .and_then(Value::as_array)
                .is_some_and(|langs| langs.iter().any(|lang| lang.as_str() == Some(key)))
        })
        .count();
    let dto = langs.get(key).unwrap_or(&Value::Null);
    LangData {
        key: key.to_string(),
        name: dto
            .get("local_name")
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_string(),
        progress: format!("{translated}/{total} translated"),
        translators: dto
            .get("translators")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        title: if key == "en" {
            TITLE.to_string()
        } else {
            format!("{TITLE} ({})", key.to_uppercase())
        },
    }
}

fn comic_item(data: LangData) -> CatalogItem {
    CatalogItem {
        key: data.key.clone(),
        title: data.title,
        authors: vec![AUTHOR.to_string()],
        description: Some(format!(
            "Language: {}\nTranslators: {}\n{}",
            data.name, data.translators, data.progress
        )),
        cover: Some(format!(
            "{BASE_URL}/0_sources/0ther/artworks/low-res/2016-02-24_vertical-cover_remake_by-David-Revoy.jpg"
        )),
        status: ItemStatus::Ongoing,
        url: Some(format!(
            "{BASE_URL}/{}/webcomics/peppercarrot.html",
            data.key
        )),
        language: Some(data.key),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn mini_fantasy_item(data: LangData) -> CatalogItem {
    CatalogItem {
        key: format!("miniFantasyTheater#{}", data.key),
        title: if data.key == "en" {
            "Mini Fantasy Theater".to_string()
        } else {
            format!("Mini Fantasy Theater ({})", data.key.to_uppercase())
        },
        authors: vec![AUTHOR.to_string()],
        description: Some(
            "A webcomic series featuring short stories set in the world of Pepper&Carrot."
                .to_string(),
        ),
        cover: Some(format!(
            "{BASE_URL}/0_sources/0ther/artworks/low-res/2018-11-22_vertical-cover-book-three_by-David-Revoy.jpg"
        )),
        status: ItemStatus::Ongoing,
        url: Some(format!(
            "{BASE_URL}/{}/webcomics/miniFantasyTheater.html",
            data.key
        )),
        language: Some(data.key),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn artwork_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: format!("#{key}"),
        title: match key {
            "comissions" => "Commissions".to_string(),
            "eshop" => "Shop".to_string(),
            _ => title_case(key),
        },
        authors: vec![AUTHOR.to_string()],
        cover: Some(format!(
            "{BASE_URL}/0_sources/0ther/press/low-res/2015-10-12_logo_by-David-Revoy.jpg"
        )),
        status: ItemStatus::Ongoing,
        url: Some(format!("{BASE_URL}/en/files/{key}.html")),
        language: Some("all".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn item_for_key(key: &str) -> Option<CatalogItem> {
    if let Some(key) = key.strip_prefix('#') {
        Some(artwork_item(key))
    } else if let Some(lang) = key.strip_prefix("miniFantasyTheater#") {
        Some(mini_fantasy_item(lang_data(lang)))
    } else if !key.contains('/') {
        Some(comic_item(lang_data(key)))
    } else {
        None
    }
}

fn chapter_list_url(key: &str) -> String {
    if let Some(key) = key.strip_prefix('#') {
        format!("{BASE_URL}/0_sources/0ther/{key}/low-res/")
    } else if let Some(lang) = key.strip_prefix("miniFantasyTheater#") {
        format!("{BASE_URL}/{lang}/webcomics/miniFantasyTheater.html")
    } else {
        format!("{BASE_URL}/{key}/webcomics/peppercarrot.html")
    }
}

fn parse_comic_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<figure")
        .skip(1)
        .filter(|chunk| chunk.contains("translated"))
        .enumerate()
        .filter_map(|(index, chunk)| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let name = html::attr_after(chunk, "<img", "title")
                .map(|value| clean_chapter_title(&value))
                .unwrap_or_else(|| format!("Episode {}", index + 1));
            Some(MangaChapter {
                key: href.trim_start_matches(BASE_URL).to_string(),
                title: Some(name),
                date_uploaded: parse_date(chunk),
                chapter_number: Some((index + 1) as f32),
                url: Some(url::join_url(BASE_URL, &href)),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_artwork_chapters(body: &str, base_url: &str) -> Vec<MangaChapter> {
    let base_dir = base_url.trim_start_matches(BASE_URL).to_string();
    let mut filenames = body
        .split("<a")
        .skip(1)
        .filter_map(|chunk| {
            let filename = html::attr(chunk, "href")?;
            filename.ends_with(".jpg").then_some(filename)
        })
        .collect::<Vec<_>>();
    filenames.reverse();
    filenames
        .into_iter()
        .map(|filename| {
            let file = filename
                .trim_end_matches(".jpg")
                .trim_end_matches("_by-David-Revoy");
            MangaChapter {
                key: format!("{base_dir}{filename}"),
                title: Some(title_case(file.trim_start_matches(|ch: char| {
                    ch.is_ascii_digit() || ch == '-' || ch == '_'
                }))),
                date_uploaded: parse_date(&filename),
                chapter_number: Some(-2.0),
                url: Some(url::join_url(BASE_URL, &format!("{base_dir}{filename}"))),
                ..MangaChapter::default()
            }
        })
        .collect()
}

fn parse_pages(body: &str, high_res: bool) -> Vec<MangaPage> {
    let mut urls = body
        .split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("webcomic-page")
                || chunk.contains("mft-cv-image")
                || chunk.contains("src=")
        })
        .filter_map(|chunk| html::attr(chunk, "src"))
        .collect::<Vec<_>>();
    if let Some(first) = urls.first() {
        if !first.to_lowercase().contains("minifantasytheater") && first.contains("P00.jpg") {
            urls.insert(0, first.replace("P00.jpg", ".jpg"));
        }
    }
    urls.into_iter()
        .enumerate()
        .map(|(index, image)| image_page(index, &url::join_url(BASE_URL, &image), high_res))
        .collect()
}

fn image_page(index: usize, image: &str, high_res: bool) -> MangaPage {
    let image = if high_res {
        image.replace("/low-res/", "/hi-res/")
    } else {
        image.to_string()
    };
    MangaPage {
        content: PageContent::Url {
            url: image,
            context: Some(manga::image_headers(BASE_URL)),
        },
        headers: manga::image_headers(BASE_URL),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }
}

fn fetch_document_or_fixture(target: &str, fixture: &str) -> String {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn fetch_json_or_fixture(target: &str, fixture: &str) -> Value {
    let body = HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string());
    serde_json::from_str(&body).unwrap_or(Value::Null)
}

fn parse_date(value: &str) -> Option<i64> {
    let index = value.find(|ch: char| ch.is_ascii_digit())?;
    let candidate = value.get(index..index + 10)?;
    let parts = candidate.split('-').collect::<Vec<_>>();
    let (year, month, day) = (
        parts.first()?.parse::<i32>().ok()?,
        parts.get(1)?.parse::<u32>().ok()?,
        parts.get(2)?.parse::<u32>().ok()?,
    );
    Some(unix_seconds_utc(year, month, day) * 1000)
}

fn unix_seconds_utc(year: i32, month: u32, day: u32) -> i64 {
    let y = year - (month <= 2) as i32;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = month as i32 + if month > 2 { -3 } else { 9 };
    let doy = (153 * m + 2) / 5 + day as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146097 + doe - 719468) as i64 * 86_400
}

fn clean_chapter_title(value: &str) -> String {
    value
        .split('（')
        .next()
        .unwrap_or(value)
        .rsplit_once('(')
        .map(|(left, _)| left)
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn title_case(value: &str) -> String {
    value
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

export_manga_source!(SOURCE);

const EPISODES_FIXTURE: &str =
    r#"[{"translated_languages":["en","fr"]},{"translated_languages":["en"]}]"#;
const LANGS_FIXTURE: &str = r#"{"en":{"translators":["David Revoy"],"local_name":"English","iso_code":"en"},"fr":{"translators":["Translator"],"local_name":"Français","iso_code":"fr"}}"#;
const COMIC_FIXTURE: &str = r#"
<figure class="translated"><a href="/en/webcomics/ep001.html"><img title="Episode 1 (English)"></a><figcaption>2014-01-01</figcaption></figure>
<figure class="translated"><a href="/en/webcomics/ep002.html"><img title="Episode 2 (English)"></a><figcaption>2014-02-01</figcaption></figure>
"#;
const ARTWORK_FIXTURE: &str = r#"<a href="2015-10-12_logo_by-David-Revoy.jpg">file</a>"#;
const PAGES_FIXTURE: &str = r#"
<div class="webcomic-page"><img src="/0_sources/ep001/low-res/en_P00.jpg"></div>
<div class="webcomic-page"><img src="/0_sources/ep001/low-res/en_P01.jpg"></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_peppercarrot_fixtures() {
        assert_eq!(selected_entries(&Value::Null).len(), 13);
        assert_eq!(parse_comic_chapters(COMIC_FIXTURE).len(), 2);
        assert_eq!(
            parse_artwork_chapters(
                ARTWORK_FIXTURE,
                "https://www.peppercarrot.com/0_sources/0ther/press/low-res/"
            )
            .len(),
            1
        );
        assert_eq!(parse_pages(PAGES_FIXTURE, false).len(), 3);
    }
}
