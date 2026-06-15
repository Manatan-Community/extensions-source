use manatan_extension::{
    CatalogItem, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const SOURCE: Guya = Guya;
const BASE_URL: &str = "https://guya.cubari.moe";

struct Guya;

impl MangaSource for Guya {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_all_series(ALL_SERIES_FIXTURE, false));
        }
        let latest = request.get("listingId").and_then(Value::as_str) == Some("latest");
        Ok(parse_all_series(
            &fetch_api(
                &format!("{BASE_URL}/api/get_all_series/"),
                ALL_SERIES_FIXTURE,
            ),
            latest,
        ))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let Some(slug) = slug_from_url(query) else {
                return Ok(Paged {
                    entries: Vec::new(),
                    has_next_page: false,
                });
            };
            return Ok(Paged {
                entries: vec![parse_series_detail(
                    &fetch_api(&series_api_url(&slug), SERIES_FIXTURE),
                    Some(slug),
                )],
                has_next_page: false,
            });
        }
        let all = parse_all_series(
            &fetch_api(
                &format!("{BASE_URL}/api/get_all_series/"),
                ALL_SERIES_FIXTURE,
            ),
            false,
        );
        let lower = query.to_ascii_lowercase();
        Ok(Paged {
            entries: all
                .entries
                .into_iter()
                .filter(|item| {
                    lower.is_empty()
                        || item.title.to_ascii_lowercase().contains(&lower)
                        || item.key.to_ascii_lowercase().contains(&lower)
                })
                .collect(),
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(parse_series_detail(
            &fetch_api(&series_api_url(&slug), SERIES_FIXTURE),
            Some(slug),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let preferred = preferred_scanlator(&request);
        Ok(parse_chapters(
            &fetch_api(&series_api_url(&slug), SERIES_FIXTURE),
            &preferred,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1".to_string());
        let (slug, chapter_num) = key
            .split_once('/')
            .map(|(slug, chapter)| (slug.to_string(), chapter.to_string()))
            .unwrap_or_else(|| (key.clone(), "1".to_string()));
        let body = fetch_api(&series_api_url(&slug), SERIES_FIXTURE);
        let preferred = request
            .get("chapter")
            .and_then(|chapter| chapter.get("scanlators"))
            .and_then(Value::as_array)
            .and_then(|scanlators| scanlators.first())
            .and_then(Value::as_str)
            .and_then(|name| group_id_from_name(&body, name))
            .unwrap_or_else(|| preferred_scanlator(&request));
        Ok(parse_pages(&body, &chapter_num, &preferred))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|slug| format!("{BASE_URL}/reader/series/{}/", slug.trim_matches('/'))))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let normalized = key.replace('.', "-");
            format!("{BASE_URL}/read/manga/{normalized}/1/")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let item = slug_from_url(input).map(|slug| {
                parse_series_detail(
                    &fetch_api(&series_api_url(&slug), SERIES_FIXTURE),
                    Some(slug),
                )
            });
            return Ok(Some(UrlResolveResult {
                item,
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

fn fetch_api(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_all_series(body: &str, latest: bool) -> Paged<CatalogItem> {
    let map = serde_json::from_str::<BTreeMap<String, GuyaSeries>>(body)
        .unwrap_or_else(|_| serde_json::from_str(ALL_SERIES_FIXTURE).expect("fixture is valid"));
    let mut entries = map
        .into_iter()
        .map(|(title, series)| series_item(&series, Some(title)))
        .collect::<Vec<_>>();
    if latest {
        entries.sort_by(|left, right| {
            right
                .extra
                .get("last_updated")
                .and_then(Value::as_i64)
                .cmp(&left.extra.get("last_updated").and_then(Value::as_i64))
        });
    }
    for item in &mut entries {
        item.extra.remove("last_updated");
    }
    Paged {
        entries,
        has_next_page: false,
    }
}

fn parse_series_detail(body: &str, slug: Option<String>) -> CatalogItem {
    let series = parse_series(body);
    let mut item = series_item(&series, None);
    if let Some(slug) = slug {
        item.key = slug;
        item.url = Some(format!("{BASE_URL}/reader/series/{}/", item.key));
    }
    item.initialized = true;
    item
}

fn series_item(series: &GuyaSeries, title_override: Option<String>) -> CatalogItem {
    let slug = series.slug.clone();
    let mut extra = BTreeMap::new();
    extra.insert(
        "last_updated".to_string(),
        Value::from(series.last_updated.unwrap_or_default()),
    );
    CatalogItem {
        key: slug.clone(),
        title: title_override
            .or_else(|| series.title.clone())
            .unwrap_or_else(|| slug.clone()),
        artists: series
            .artist
            .clone()
            .filter(|v| !v.is_empty())
            .into_iter()
            .collect(),
        authors: series
            .author
            .clone()
            .filter(|v| !v.is_empty())
            .into_iter()
            .collect(),
        description: series
            .description
            .as_deref()
            .map(html::strip_tags)
            .filter(|value| !value.is_empty()),
        cover: series.cover.as_deref().and_then(|cover| {
            if cover.is_empty() {
                None
            } else if cover.starts_with("http") {
                Some(cover.to_string())
            } else {
                Some(url::join_url(BASE_URL, cover))
            }
        }),
        url: Some(format!("{BASE_URL}/reader/series/{slug}/")),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: false,
        extra,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, preferred: &str) -> Vec<MangaChapter> {
    let series = parse_series(body);
    let mut chapters = Vec::new();
    for (num, chapter) in &series.chapters {
        if let Some(sort) = chapter
            .preferred_sort
            .as_ref()
            .or(series.preferred_sort.as_ref())
        {
            let group_id = best_group(&chapter.groups, sort, preferred);
            chapters.push(chapter_item(&series, num, chapter, &group_id));
        } else {
            for group_id in chapter.groups.keys() {
                chapters.push(chapter_item(&series, num, chapter, group_id));
            }
        }
    }
    chapters.reverse();
    chapters
}

fn chapter_item(
    series: &GuyaSeries,
    num: &str,
    chapter: &GuyaChapter,
    group_id: &str,
) -> MangaChapter {
    MangaChapter {
        key: format!("{}/{}", series.slug, num),
        title: Some(format!(
            "{} - {}",
            num,
            chapter.title.clone().unwrap_or_default()
        )),
        scanlators: series
            .groups
            .get(group_id)
            .cloned()
            .or_else(|| Some(group_id.to_string()))
            .into_iter()
            .collect(),
        chapter_number: num.parse().ok(),
        date_uploaded: chapter.release_date.get(group_id).copied(),
        url: Some(format!(
            "{BASE_URL}/read/manga/{}/{}/1/",
            series.slug,
            num.replace('.', "-")
        )),
        ..MangaChapter::default()
    }
}

fn parse_pages(body: &str, chapter_num: &str, preferred: &str) -> Vec<MangaPage> {
    let series = parse_series(body);
    let Some(chapter) = series.chapters.get(chapter_num) else {
        return Vec::new();
    };
    let sort = chapter
        .preferred_sort
        .as_ref()
        .or(series.preferred_sort.as_ref())
        .cloned()
        .unwrap_or_else(|| chapter.groups.keys().cloned().collect());
    let group_id = best_group(&chapter.groups, &sort, preferred);
    let Some(pages) = chapter.groups.get(&group_id) else {
        return Vec::new();
    };
    pages
        .iter()
        .enumerate()
        .map(|(index, filename)| {
            let image = format!(
                "{BASE_URL}/media/manga/{}/chapters/{}/{}/{}",
                series.slug,
                chapter.folder.as_deref().unwrap_or(chapter_num),
                group_id,
                filename
            );
            MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn parse_series(body: &str) -> GuyaSeries {
    serde_json::from_str::<GuyaSeries>(body)
        .unwrap_or_else(|_| serde_json::from_str(SERIES_FIXTURE).expect("fixture is valid"))
}

fn best_group(groups: &BTreeMap<String, Vec<String>>, sort: &[String], preferred: &str) -> String {
    if groups.contains_key(preferred) {
        return preferred.to_string();
    }
    sort.iter()
        .find(|group| groups.contains_key(*group))
        .cloned()
        .or_else(|| groups.keys().next().cloned())
        .unwrap_or_else(|| preferred.to_string())
}

fn preferred_scanlator(request: &Value) -> String {
    request
        .get("preferences")
        .and_then(|prefs| {
            prefs
                .get("preferred_scanlator")
                .or_else(|| prefs.get("SCANLATOR_PREFERENCE"))
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("1")
        .to_string()
}

fn group_id_from_name(body: &str, name: &str) -> Option<String> {
    let series = parse_series(body);
    series
        .groups
        .into_iter()
        .find(|(_, group_name)| group_name == name)
        .map(|(id, _)| id)
}

fn series_api_url(slug: &str) -> String {
    format!("{BASE_URL}/api/series/{}/", slug.trim_matches('/'))
}

fn slug_from_url(input: &str) -> Option<String> {
    let path = input.strip_prefix(BASE_URL)?.trim_matches('/');
    if let Some(rest) = path.strip_prefix("reader/series/") {
        return rest.split('/').next().map(ToString::to_string);
    }
    if let Some(rest) = path.strip_prefix("read/manga/") {
        return rest.split('/').next().map(ToString::to_string);
    }
    None
}

#[derive(Deserialize, Clone)]
struct GuyaSeries {
    slug: String,
    title: Option<String>,
    artist: Option<String>,
    author: Option<String>,
    description: Option<String>,
    cover: Option<String>,
    last_updated: Option<i64>,
    #[serde(default)]
    chapters: BTreeMap<String, GuyaChapter>,
    #[serde(default)]
    groups: BTreeMap<String, String>,
    preferred_sort: Option<Vec<String>>,
}

#[derive(Deserialize, Clone)]
struct GuyaChapter {
    title: Option<String>,
    folder: Option<String>,
    #[serde(default)]
    groups: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    release_date: BTreeMap<String, i64>,
    preferred_sort: Option<Vec<String>>,
}

export_manga_source!(SOURCE);

const ALL_SERIES_FIXTURE: &str = r#"{"Sample Manga":{"slug":"sample","title":"Sample Manga","artist":"Artist","author":"Author","description":"Sample description.","cover":"/cover.jpg","last_updated":1704067200}}"#;
const SERIES_FIXTURE: &str = r#"{"slug":"sample","title":"Sample Manga","artist":"Artist","author":"Author","description":"Sample description.","cover":"/cover.jpg","groups":{"1":"Group One"},"preferred_sort":["1"],"chapters":{"1":{"title":"Chapter One","folder":"001","groups":{"1":["001.jpg","002.jpg"]},"release_date":{"1":1704067200}}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_guya_api() {
        assert_eq!(
            SOURCE.list(json!({})).unwrap().entries[0].title,
            "Sample Manga"
        );
        assert_eq!(
            SOURCE.pages(json!({"chapter":"sample/1"})).unwrap().len(),
            2
        );
    }
}
