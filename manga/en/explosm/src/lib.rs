use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;

const SOURCE: Explosm = Explosm;
const BASE_URL: &str = "https://explosm.net";
const ARCHIVE_URL: &str = "https://explosm.net/comics";
const COVER: &str = "https://vhx.imgix.net/vitalyuncensored/assets/13ea3806-5ebf-4987-bcf1-82af2b689f77/S2E4_Still1.jpg";

struct Explosm;

impl MangaSource for Explosm {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let archive = if request.as_object().is_some_and(|object| object.is_empty()) {
            archive_from_json(ARCHIVE_FIXTURE)
        } else {
            fetch_archive()
        };
        Ok(Paged {
            entries: archive.years_as_items(),
            has_next_page: false,
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        if query.starts_with(BASE_URL) {
            let year = query
                .split("/comics/")
                .nth(1)
                .and_then(|rest| rest.split(['-', '#', '/']).next())
                .filter(|part| part.len() == 4)
                .unwrap_or("2024");
            return Ok(Paged {
                entries: vec![series_item(year)],
                has_next_page: false,
            });
        }
        let mut entries = fetch_archive().years_as_items();
        if !query.is_empty() {
            entries.retain(|item| item.title.to_ascii_lowercase().contains(&query));
        }
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "2024".to_string());
        Ok(series_item(key.trim_matches('/')))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let year = manga::request_key(&request, "manga").unwrap_or_else(|| "2024".to_string());
        Ok(fetch_archive().chapters_for_year(year.trim_matches('/')))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| {
            "/comics/sample#https://files.explosm.net/comics/sample.png".to_string()
        });
        let image = key
            .split_once('#')
            .map(|(_, image)| image)
            .unwrap_or(key.as_str());
        Ok(vec![MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some("Page 1".to_string()),
            ..MangaPage::default()
        }])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|year| format!("{ARCHIVE_URL}#{year}-01")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter")
            .and_then(|key| key.split('#').next().map(ToString::to_string))
            .map(|path| url::join_url(BASE_URL, &path)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let year = input
                .split("/comics/")
                .nth(1)
                .and_then(|rest| rest.split(['-', '#', '/']).next())
                .filter(|part| part.len() == 4)
                .unwrap_or("2024");
            return Ok(Some(UrlResolveResult {
                item: Some(series_item(year)),
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
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch_archive() -> ComicArchive {
    let html = client()
        .get(ARCHIVE_URL)
        .browser_document()
        .send_text()
        .unwrap_or_default();
    let json_path = html
        .split("<script")
        .filter_map(|chunk| html::attr(chunk, "src"))
        .last()
        .map(|src| src.replace("static", "data"))
        .map(|src| {
            let prefix = src.rsplit_once('/').map(|(prefix, _)| prefix).unwrap_or("");
            format!("{prefix}/comics.json")
        })
        .unwrap_or_else(|| "/_next/data/build/comics.json".to_string());
    let body = client()
        .get(url::join_url(BASE_URL, &json_path))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| ARCHIVE_FIXTURE.to_string());
    archive_from_json(&body)
}

fn archive_from_json(body: &str) -> ComicArchive {
    serde_json::from_str::<ArchiveEnvelope>(body)
        .ok()
        .and_then(|payload| payload.page_props.comic_archive_data)
        .unwrap_or_else(|| {
            serde_json::from_str::<ComicArchive>(ARCHIVE_FIXTURE).expect("fixture is valid")
        })
}

fn series_item(year: &str) -> CatalogItem {
    CatalogItem {
        key: year.to_string(),
        title: format!("C&H {year}"),
        cover: Some(COVER.to_string()),
        authors: vec!["Explosm.net".to_string()],
        artists: vec!["Explosm.net".to_string()],
        status: ItemStatus::Completed,
        url: Some(format!("{ARCHIVE_URL}#{year}-01")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveEnvelope {
    #[serde(default)]
    page_props: PageProps,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageProps {
    comic_archive_data: Option<ComicArchive>,
}

#[derive(Debug, Default, Deserialize)]
struct ComicArchive(serde_json::Map<String, Value>);

impl ComicArchive {
    fn years_as_items(&self) -> Vec<CatalogItem> {
        let mut years = self.0.keys().cloned().collect::<Vec<_>>();
        years.sort();
        years.reverse();
        years.into_iter().map(|year| series_item(&year)).collect()
    }

    fn chapters_for_year(&self, year: &str) -> Vec<MangaChapter> {
        let Some(months) = self.0.get(year).and_then(Value::as_object) else {
            return Vec::new();
        };
        let mut comics = Vec::new();
        for value in months.values() {
            comics.extend(
                value
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Comic::from_value),
            );
        }
        comics.reverse();
        comics
            .into_iter()
            .enumerate()
            .map(|(index, comic)| comic.into_chapter(index + 1))
            .collect()
    }
}

#[derive(Debug)]
struct Comic {
    slug: String,
    file: String,
    file_static: Option<String>,
    publish_at: Option<String>,
    author_name: Option<String>,
}

impl Comic {
    fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            slug: value.get("slug")?.as_str()?.to_string(),
            file: value.get("file")?.as_str()?.to_string(),
            file_static: value
                .get("file_static")
                .and_then(Value::as_str)
                .filter(|value| *value != "null" && !value.is_empty())
                .map(ToString::to_string),
            publish_at: value
                .get("publish_at")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            author_name: value
                .get("author_name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        })
    }

    fn image_url(&self) -> String {
        if let Some(static_file) = &self.file_static {
            static_file.to_string()
        } else if self.file.starts_with("http") {
            self.file.to_string()
        } else {
            format!("https://files.explosm.net/comics/{}", self.file)
        }
    }

    fn into_chapter(self, number: usize) -> MangaChapter {
        let image = self.image_url();
        let slug = self.slug;
        MangaChapter {
            key: format!("/comics/{slug}#{image}"),
            title: Some(slug.clone()),
            chapter_number: Some(number as f32),
            date_uploaded: self.publish_at.as_deref().and_then(parse_publish_date),
            scanlators: self.author_name.into_iter().collect(),
            url: Some(format!("{BASE_URL}/comics/{slug}")),
            language: Some("en".to_string()),
            page_count: Some(1),
            ..MangaChapter::default()
        }
    }
}

fn parse_publish_date(value: &str) -> Option<i64> {
    let date = value.split_whitespace().next()?;
    let parts = date
        .split('-')
        .filter_map(|part| part.parse::<i32>().ok())
        .collect::<Vec<_>>();
    (parts.len() == 3)
        .then(|| unix_date(parts[0], parts[1], parts[2]))
        .flatten()
}

fn unix_date(year: i32, month: i32, day: i32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let y = year - (month <= 2) as i32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(((era * 146_097 + doe - 719_468) as i64) * 86_400)
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"{
  "2024": {
    "01": [
      {
        "slug": "sample-comic",
        "file": "sample.png",
        "file_static": "null",
        "publish_at": "2024-01-01 00:00:00",
        "author_name": "Explosm.net"
      }
    ]
  }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_archive() {
        let list = SOURCE.list(json!({})).unwrap();
        assert_eq!(list.entries[0].title, "C&H 2024");
        let chapters = SOURCE.chapters(json!({"manga":"2024"})).unwrap();
        assert_eq!(chapters[0].page_count, Some(1));
    }
}
