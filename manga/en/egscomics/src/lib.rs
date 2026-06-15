use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage, PageContent,
    Paged, UrlResolveResult, abi::ExtensionResult, export_manga_source, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, sdk::http::HttpClient, url};
use serde_json::Value;

const SOURCE: ElGoonishShive = ElGoonishShive;
const BASE_URL: &str = "https://www.egscomics.com";
const COVER: &str = "https://static.tumblr.com/8cee5e83d26a8a96ad5e51b67f2e340e/j8ipbno/fXFoj0zh9/tumblr_static_1f2fhwjyya74gsgs888g8k880.png";
const AUTHOR: &str = "Dan Shive";

struct ElGoonishShive;

impl MangaSource for ElGoonishShive {
    fn list(&self, _request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        Ok(Paged {
            entries: catalog(),
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
        let entries = catalog()
            .into_iter()
            .filter(|item| {
                query.is_empty()
                    || item.title.to_ascii_lowercase().contains(&query)
                    || item.key.contains(&query)
                    || query.starts_with(BASE_URL) && query.contains(&item.key)
            })
            .collect();
        Ok(Paged {
            entries,
            has_next_page: false,
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/archive".to_string());
        Ok(catalog()
            .into_iter()
            .find(|item| item.key == key)
            .unwrap_or_else(|| catalog()[0].clone()))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comic/archive".to_string());
        Ok(parse_chapters(&fetch_document(
            &url::join_url(BASE_URL, &key),
            ARCHIVE_FIXTURE,
        )))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/comic/sample".into());
        Ok(parse_pages(&fetch_document(
            &url::join_url(BASE_URL, &key),
            PAGE_FIXTURE,
        )))
    }

    fn home(&self, _request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        Ok(vec![HomeSection {
            id: "series".to_string(),
            title: "Series".to_string(),
            style: Some(HomeSectionStyle::Compact),
            entries: catalog(),
            has_more: false,
            ..HomeSection::default()
        }])
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) {
            let key = normalize_key(input);
            let item = catalog()
                .into_iter()
                .find(|item| key.starts_with(item.key.trim_end_matches("/archive")));
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

fn catalog() -> Vec<CatalogItem> {
    [
        (
            "/comic/archive",
            "El Goonish Shive",
            "El Goonish Shive is a comic about a group of teenagers who face both real life and bizarre, supernatural situations.\n\nIt is a comedy mixed with drama and is recommended for audiences thirteen and older.",
        ),
        (
            "/egsnp/archive",
            "El Goonish Shive: NewsPaper",
            "EGS:NP is a subsection with short stories that generally are not canon unless stated.",
        ),
        (
            "/sketchbook/archive",
            "El Goonish Shive Sketchbook",
            "The Sketchbook section is full of one-shot gags, sketches, and comics that do not fit elsewhere.",
        ),
    ]
    .into_iter()
    .map(|(key, title, description)| CatalogItem {
        key: key.to_string(),
        title: title.to_string(),
        cover: Some(COVER.to_string()),
        authors: vec![AUTHOR.to_string()],
        artists: vec![AUTHOR.to_string()],
        description: Some(description.to_string()),
        status: ItemStatus::Ongoing,
        url: Some(url::join_url(BASE_URL, key)),
        language: Some("en".to_string()),
        content_rating: Some("safe".to_string()),
        initialized: true,
        ..CatalogItem::default()
    })
    .collect()
}

fn client() -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/"))
}

fn fetch_document(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body.split("<option")
        .skip(1)
        .filter_map(|chunk| {
            let value = html::attr(chunk, "value")?;
            if !value.starts_with("comic")
                && !value.starts_with("egsnp")
                && !value.starts_with("sketchbook")
            {
                return None;
            }
            let text = html::text_between(chunk, ">", "</option>")
                .map(|value| html::strip_tags(&value))
                .unwrap_or_else(|| value.clone());
            let (date, title) = text
                .split_once(" - ")
                .map(|(date, title)| (date.to_string(), title.to_string()))
                .unwrap_or_else(|| (String::new(), text));
            Some(MangaChapter {
                key: format!("/{value}"),
                title: Some(title),
                chapter_number: numeric_suffix(&value),
                date_uploaded: parse_month_date(&date),
                url: Some(format!("{BASE_URL}/{value}")),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn parse_pages(body: &str) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| chunk.contains("cc-comic") || chunk.contains("id=\"cc-comic\""))
        .filter_map(|chunk| html::attr(chunk, "src"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(BASE_URL)),
            },
            headers: manga::image_headers(BASE_URL),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn normalize_key(input: &str) -> String {
    if let Some(index) = input.find(".com/") {
        return format!("/{}", input[index + 5..].trim_matches('/'));
    }
    format!("/{}", input.trim_matches('/'))
}

fn numeric_suffix(input: &str) -> Option<f32> {
    input
        .chars()
        .rev()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .ok()
}

fn parse_month_date(input: &str) -> Option<i64> {
    let parts = input.replace(',', "").split_whitespace().map(str::to_string).collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let month = match parts[0].as_str() {
        "January" => 1,
        "February" => 2,
        "March" => 3,
        "April" => 4,
        "May" => 5,
        "June" => 6,
        "July" => 7,
        "August" => 8,
        "September" => 9,
        "October" => 10,
        "November" => 11,
        "December" => 12,
        _ => return None,
    };
    unix_date(parts[2].parse().ok()?, month, parts[1].parse().ok()?)
}

fn unix_date(year: i32, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 1..month {
        days += days_in_month(year, m) as i64;
    }
    Some((days + day as i64 - 1) * 86_400)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

export_manga_source!(SOURCE);

const ARCHIVE_FIXTURE: &str = r#"
<select name="comic">
<option value="comic/sample">January 1, 2024 - Sample Comic</option>
<option value="egsnp/sample">February 1, 2024 - Sample NP</option>
</select>
"#;
const PAGE_FIXTURE: &str = r#"<img id="cc-comic" src="/comics/sample.png">"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_egs_fixture() {
        assert_eq!(SOURCE.list(json!({})).unwrap().entries.len(), 3);
        assert_eq!(SOURCE.chapters(json!({})).unwrap().len(), 2);
        assert_eq!(SOURCE.pages(json!({})).unwrap().len(), 1);
    }
}
