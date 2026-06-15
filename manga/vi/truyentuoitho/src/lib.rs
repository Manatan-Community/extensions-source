use base64::{Engine as _, engine::general_purpose::STANDARD};
use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, MangaChapter, MangaPage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{
    html,
    manga::{self, Madara, MadaraConfig, MadaraSource},
    url,
};
use serde_json::{Value, json};

const SOURCE: TruyenTuoiTho = TruyenTuoiTho;

struct TruyenTuoiTho;

impl MadaraSource for TruyenTuoiTho {
    fn madara_config(&self, _request: &Value) -> MadaraConfig {
        MadaraConfig {
            base_url: "https://truyentuoitho.com",
            lang: "vi",
            content_rating: "safe",
            manga_path: "manga",
            popular_url_marker: "post-title",
            use_load_more: false,
            latest_enabled: true,
        }
    }

    fn madara_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn madara_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn madara_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl MangaSource for TruyenTuoiTho {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.madara_list(request)
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        self.madara_search(request)
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        self.madara_details(request)
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = self.madara_config(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| self.madara_default_manga_key(&config));
        Ok(fetch_new_endpoint_chapters(&config, &key))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = self.madara_config(&request);
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| self.madara_default_chapter_key(&config));
        let chapter_url = config.absolute_url(&key);
        let body = Madara::fetch_document_or_fixture(&config, &chapter_url, PAGES_FIXTURE);
        let default_pages = Madara::parse_pages(&body, &config);
        if !default_pages.is_empty() {
            return Ok(default_pages);
        }
        Ok(parse_protected_pages(&body, &chapter_url, config.base_url))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![
            home_section(
                "popular",
                "Popular",
                self.list(json!({"page": 1, "listingId": "popular"}))?,
            ),
            home_section(
                "latest",
                "Latest",
                self.list(json!({"page": 1, "listingId": "latest"}))?,
            ),
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = self.madara_config(&request);
        Ok(manga::request_key(&request, "manga").map(|key| config.absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = self.madara_config(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| config.absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        self.madara_handle_url(request)
    }
}

fn fetch_new_endpoint_chapters(config: &MadaraConfig, manga_key: &str) -> Vec<MangaChapter> {
    let manga_url = config
        .absolute_url(manga_key)
        .trim_end_matches('/')
        .to_string();
    let body = Madara::browser_client(config)
        .post(format!("{manga_url}/ajax/chapters/?t=1"))
        .xhr()
        .referer(format!("{manga_url}/"))
        .origin(config.base_url)
        .send_text()
        .unwrap_or_else(|_| CHAPTERS_FIXTURE.to_string());
    parse_chapters(&body, config)
}

fn parse_chapters(body: &str, config: &MadaraConfig) -> Vec<MangaChapter> {
    body.split("<li")
        .skip(1)
        .filter(|chunk| chunk.contains("wp-manga-chapter"))
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let title = html::text_between(chunk, "<a", "</a>")
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapter".to_string());
            let key = normalize_chapter_key(config, &href);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title),
                date_uploaded: html::text_between(chunk, "chapter-release-date", "</")
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_dd_mm_yyyy(&value)),
                url: Some(config.absolute_url(&key)),
                ..MangaChapter::default()
            })
        })
        .collect()
}

fn parse_protected_pages(body: &str, chapter_url: &str, base_url: &str) -> Vec<MangaPage> {
    body.split("<script")
        .skip(1)
        .filter_map(|chunk| decode_protected_payload(chunk).ok())
        .find_map(|payload| images_from_payload(&payload))
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(base_url, &image),
                context: Some(manga::image_headers(chapter_url)),
            },
            headers: manga::image_headers(chapter_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn decode_protected_payload(script: &str) -> Result<String, ()> {
    if !script.contains("split('').reverse().join('')")
        || !script.contains("JSON[atob('cGFyc2U=')]")
    {
        return Err(());
    }
    let (parts, cursor) = atob_parts(script, 4)?;
    let encrypted = quoted_assignment_after(&script[cursor..]).ok_or(())?;
    let key = parts
        .iter()
        .filter_map(|part| decode_base64_text(part).ok())
        .collect::<String>();
    if key.is_empty() {
        return Err(());
    }
    let reversed = encrypted.chars().rev().collect::<String>();
    let decoded = decode_base64_text(&reversed)?;
    Ok(xor_with_key(&decoded, &key))
}

fn atob_parts(input: &str, count: usize) -> Result<(Vec<String>, usize), ()> {
    let mut parts = Vec::new();
    let mut cursor = 0;
    while parts.len() < count {
        let Some(start) = input[cursor..].find("atob('") else {
            return Err(());
        };
        cursor += start + "atob('".len();
        let Some(end) = input[cursor..].find("')") else {
            return Err(());
        };
        parts.push(input[cursor..cursor + end].to_string());
        cursor += end + 2;
    }
    Ok((parts, cursor))
}

fn quoted_assignment_after(input: &str) -> Option<String> {
    let equals = input.find('=')?;
    let tail = input[equals + 1..].trim_start();
    let start = tail.strip_prefix('\'')?;
    let end = start.find('\'')?;
    Some(start[..end].to_string())
}

fn images_from_payload(payload: &str) -> Option<Vec<String>> {
    let value: Value = serde_json::from_str(payload).ok()?;
    Some(
        value
            .get("images")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect(),
    )
}

fn decode_base64_text(input: &str) -> Result<String, ()> {
    let bytes = STANDARD.decode(input).map_err(|_| ())?;
    String::from_utf8(bytes).map_err(|_| ())
}

fn xor_with_key(input: &str, key: &str) -> String {
    input
        .bytes()
        .zip(key.bytes().cycle())
        .map(|(byte, key_byte)| (byte ^ key_byte) as char)
        .collect()
}

fn normalize_chapter_key(config: &MadaraConfig, href: &str) -> String {
    config
        .normalize_manga_key(href)
        .trim_end_matches("?style=paged")
        .trim_end_matches("?style=list")
        .to_string()
}

fn parse_dd_mm_yyyy(value: &str) -> Option<i64> {
    let mut parts = value.trim().split('/');
    let day = parts.next()?.parse::<u32>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let year = parts.next()?.parse::<i32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    manatan_shared::dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn home_section(id: &str, title: &str, page: Paged<CatalogItem>) -> HomeSection<CatalogItem> {
    HomeSection {
        id: id.into(),
        title: title.into(),
        style: Some(HomeSectionStyle::Cover),
        has_more: page.has_next_page,
        entries: page.entries,
        ..HomeSection::default()
    }
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="page-item-detail"><div class="post-title"><a href="/manga/sample/">Sample</a></div><img src="/cover.jpg"></div>
"#;
const DETAILS_FIXTURE: &str = r#"
<div class="post-title"><h1>Sample</h1></div><div class="summary_image"><img src="/cover.jpg"></div>
<div class="description-summary">Summary</div><div id="manga-chapters-holder" data-id="1"></div>
"#;
const CHAPTERS_FIXTURE: &str = r#"
<li class="wp-manga-chapter"><a href="/manga/sample/chapter-1/">Chapter 1</a><span class="chapter-release-date">01/01/2024</span></li>
"#;
const PAGES_FIXTURE: &str =
    r#"<div class="reading-content"><img class="wp-manga-chapter-img" src="/page1.jpg"></div>"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_ajax_chapters_and_pages() {
        let chapters = SOURCE.chapters(json!({"manga": "/manga/sample"})).unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));

        let pages = SOURCE
            .pages(json!({"chapter": "/manga/sample/chapter-1"}))
            .unwrap();
        assert_eq!(pages.len(), 1);
    }
}
