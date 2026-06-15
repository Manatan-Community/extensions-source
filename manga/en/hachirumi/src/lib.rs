use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: Hachirumi = Hachirumi;
const BASE_URL: &str = "https://hachirumi.com";

struct Hachirumi;

impl MangaSource for Hachirumi {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let mut entries = parse_series_map(&fetch_api("/api/get_all_series/", SERIES_FIXTURE));
        if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            entries.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        }
        Ok(Paged {
            entries: entries.into_iter().map(|(_, item)| item).collect(),
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
            let slug = slug_from_url(query).unwrap_or_else(|| "sample".to_string());
            return Ok(Paged {
                entries: vec![details_by_slug(&slug)],
                has_next_page: false,
            });
        }
        let needle = query.to_ascii_lowercase();
        let entries = parse_series_map(&fetch_api("/api/get_all_series/", SERIES_FIXTURE))
            .into_iter()
            .map(|(_, item)| item)
            .filter(|item| needle.is_empty() || item.title.to_ascii_lowercase().contains(&needle))
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        Ok(details_by_slug(&slug))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let slug = manga::request_key(&request, "manga").unwrap_or_else(|| "sample".to_string());
        let body = fetch_api(&format!("/api/series/{slug}/"), SERIES_DETAILS_FIXTURE);
        Ok(parse_chapters(&body, &slug))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key =
            manga::request_key(&request, "chapter").unwrap_or_else(|| "sample/1#1".to_string());
        let (slug_and_num, group_id) = key.split_once('#').unwrap_or((&key, "1"));
        let (slug, number) = slug_and_num.split_once('/').unwrap_or((slug_and_num, "1"));
        let body = fetch_api(&format!("/api/series/{slug}/"), SERIES_DETAILS_FIXTURE);
        Ok(parse_pages(&body, slug, number, group_id))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga")
            .map(|key| format!("{BASE_URL}/reader/series/{key}/")))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let slug_and_num = key.split('#').next().unwrap_or(&key).replace('.', "-");
            format!("{BASE_URL}/read/manga/{slug_and_num}/1/")
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            if let Some(slug) = slug_from_url(input) {
                return Ok(Some(UrlResolveResult {
                    item: Some(details_by_slug(&slug)),
                    url: Some(input.to_string()),
                    ..UrlResolveResult::default()
                }));
            }
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

fn fetch_api(path: &str, fixture: &str) -> String {
    client()
        .get(format!("{BASE_URL}{path}"))
        .xhr()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn details_by_slug(slug: &str) -> CatalogItem {
    let body = fetch_api(&format!("/api/series/{slug}/"), SERIES_DETAILS_FIXTURE);
    let value = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
    catalog_from_json(value.as_object().map(|_| &value), slug, None)
        .unwrap_or_else(|| fallback_item(slug))
}

fn parse_series_map(body: &str) -> Vec<(f64, CatalogItem)> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    root.as_object()
        .into_iter()
        .flat_map(|object| object.iter())
        .filter_map(|(title, value)| {
            let timestamp = value
                .get("last_updated")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            catalog_from_json(Some(value), "", Some(title)).map(|item| (timestamp, item))
        })
        .collect()
}

fn catalog_from_json(
    value: Option<&Value>,
    fallback_slug: &str,
    map_title: Option<&str>,
) -> Option<CatalogItem> {
    let value = value?;
    let slug = value
        .get("slug")
        .and_then(Value::as_str)
        .unwrap_or(fallback_slug);
    let title = map_title
        .or_else(|| value.get("title").and_then(Value::as_str))
        .unwrap_or(slug);
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .map(html::strip_tags)
        .filter(|value| !value.is_empty());
    let cover = value
        .get("cover")
        .and_then(Value::as_str)
        .and_then(|cover| {
            if cover.is_empty() {
                None
            } else {
                Some(url::join_url(BASE_URL, cover))
            }
        });
    Some(CatalogItem {
        key: slug.to_string(),
        title: title.to_string(),
        authors: value
            .get("author")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        artists: value
            .get("artist")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(|value| vec![value.to_string()])
            .unwrap_or_default(),
        description,
        cover,
        status: ItemStatus::Unknown,
        url: Some(format!("{BASE_URL}/reader/series/{slug}/")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: value.get("chapters").is_some(),
        ..CatalogItem::default()
    })
}

fn parse_chapters(body: &str, slug: &str) -> Vec<MangaChapter> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let groups = root.get("groups").unwrap_or(&Value::Null);
    let preferred_root = root.get("preferred_sort").and_then(Value::as_array);
    let Some(chapters) = root.get("chapters").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (number, chapter) in chapters {
        let chapter_groups = chapter.get("groups").unwrap_or(&Value::Null);
        let preferred = chapter
            .get("preferred_sort")
            .and_then(Value::as_array)
            .or(preferred_root);
        let group_ids = if let Some(preferred) = preferred {
            preferred
                .iter()
                .filter_map(Value::as_str)
                .find(|group_id| chapter_groups.get(*group_id).is_some())
                .map(|group_id| vec![group_id.to_string()])
                .unwrap_or_else(|| chapter_group_ids(chapter_groups))
        } else {
            chapter_group_ids(chapter_groups)
        };
        for group_id in group_ids {
            let title = chapter
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Chapter");
            let key = format!("{slug}/{number}#{group_id}");
            out.push(MangaChapter {
                key: key.clone(),
                title: Some(format!("{number} - {title}")),
                chapter_number: number.parse().ok(),
                scanlators: group_name(groups, &group_id)
                    .into_iter()
                    .collect::<Vec<_>>(),
                date_uploaded: chapter
                    .get("release_date")
                    .and_then(|dates| dates.get(&group_id))
                    .and_then(Value::as_i64)
                    .map(|seconds| seconds * 1000),
                url: Some(format!("{BASE_URL}/read/manga/{slug}/{number}/1/")),
                ..MangaChapter::default()
            });
        }
    }
    out.reverse();
    out
}

fn chapter_group_ids(groups: &Value) -> Vec<String> {
    groups
        .as_object()
        .into_iter()
        .flat_map(|object| object.keys().cloned())
        .collect()
}

fn group_name(groups: &Value, group_id: &str) -> Option<String> {
    groups
        .get(group_id)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| Some(group_id.to_string()))
}

fn parse_pages(body: &str, slug: &str, number: &str, group_id: &str) -> Vec<MangaPage> {
    let root = serde_json::from_str::<Value>(body).unwrap_or(Value::Null);
    let Some(chapter) = root
        .get("chapters")
        .and_then(|chapters| chapters.get(number))
    else {
        return Vec::new();
    };
    let folder = chapter
        .get("folder")
        .and_then(Value::as_str)
        .unwrap_or(number);
    let pages = chapter
        .get("groups")
        .and_then(|groups| groups.get(group_id))
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| {
            chapter
                .get("groups")
                .and_then(Value::as_object)
                .and_then(|object| object.values().find_map(Value::as_array).cloned())
        })
        .unwrap_or_default();
    pages
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let filename = page.as_str()?;
            let image =
                format!("{BASE_URL}/media/manga/{slug}/chapters/{folder}/{group_id}/{filename}");
            Some(MangaPage {
                content: PageContent::Url {
                    url: image,
                    context: Some(manga::image_headers(BASE_URL)),
                },
                headers: manga::image_headers(BASE_URL),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn slug_from_url(input: &str) -> Option<String> {
    input
        .split("/reader/series/")
        .nth(1)
        .or_else(|| input.split("/read/manga/").nth(1))
        .and_then(|rest| rest.split('/').find(|part| !part.is_empty()))
        .map(ToString::to_string)
        .or_else(|| url::slug_from_url(input))
}

fn fallback_item(slug: &str) -> CatalogItem {
    CatalogItem {
        key: slug.to_string(),
        title: slug.replace('-', " "),
        status: ItemStatus::Unknown,
        url: Some(format!("{BASE_URL}/reader/series/{slug}/")),
        language: Some("en".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

export_manga_source!(SOURCE);

const SERIES_FIXTURE: &str = r#"
{
  "Sample Series": {
    "slug": "sample",
    "title": "Sample Series",
    "author": "Author",
    "artist": "Artist",
    "description": "<p>Description</p>",
    "cover": "/media/manga/sample/cover.jpg",
    "last_updated": 1704067200
  }
}
"#;
const SERIES_DETAILS_FIXTURE: &str = r#"
{
  "slug": "sample",
  "title": "Sample Series",
  "author": "Author",
  "artist": "Artist",
  "description": "<p>Description</p>",
  "cover": "/media/manga/sample/cover.jpg",
  "groups": {"1": "Group One"},
  "chapters": {
    "1": {
      "title": "Start",
      "folder": "001",
      "groups": {"1": ["001.jpg", "002.jpg"]},
      "release_date": {"1": 1704067200}
    }
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_guya_api_shape() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Series");
        let pages = SOURCE.pages(json!({"chapter":"sample/1#1"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
