// Adapted from LNReader/lnreader-plugins under the MIT license.

use chrono::DateTime;
use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequestContext, NovelChapter, NovelContentBlock,
        NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Map, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "kakuyomu";
const BASE_URL: &str = "https://kakuyomu.jp";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

pub struct KakuyomuSource {
    client: Client,
}

impl Default for KakuyomuSource {
    fn default() -> Self {
        Self {
            client: Client::new()
                .header("User-Agent", USER_AGENT)
                .header("Referer", BASE_URL),
        }
    }
}

impl KakuyomuSource {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok((
            html::document(response.text()?),
            response.final_url().to_owned(),
        ))
    }

    fn ranking_url(page: u32, filters: &Value) -> Result<String> {
        let genre = filter(filters, "genre", "all");
        let period = filter(filters, "period", "entire");
        let mut url = Url::parse(&format!("{BASE_URL}/rankings/{genre}/{period}"))
            .map_err(|error| Error::new(error.to_string()))?;
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(url.to_string())
    }

    fn parse_listing(document: &Html) -> Result<Paged<CatalogItem>> {
        let rows = selector(".widget-media-genresWorkList-right > .widget-work")?;
        let title = selector("a.widget-workCard-titleLabel")?;
        let next = selector(".widget-pagerNext, .widget-pagerNext a")?;
        let mut entries = Vec::new();
        for row in document.select(&rows) {
            let Some(anchor) = row.select(&title).next() else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let name = normalize_space(&html::text(anchor));
            if name.is_empty() {
                continue;
            }
            let url = absolute_url(BASE_URL, &href)?;
            let mut item = CatalogItem::new(url.clone(), name);
            item.url = Some(url);
            item.language = Some("ja".into());
            entries.push(item);
        }
        Ok(Paged::new(entries, document.select(&next).next().is_some()))
    }

    fn apollo_state(document: &Html) -> Result<Map<String, Value>> {
        let data = selector("script#__NEXT_DATA__[type=\"application/json\"]")?;
        let json = document
            .select(&data)
            .next()
            .map(|element| element.inner_html())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::new("Kakuyomu page has no __NEXT_DATA__ payload"))?;
        let root: Value = serde_json::from_str(&json)?;
        root.pointer("/props/pageProps/__APOLLO_STATE__")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| Error::new("Kakuyomu page has no Apollo state"))
    }

    fn parse_search(document: &Html) -> Result<Paged<CatalogItem>> {
        let state = Self::apollo_state(document)?;
        let next = selector(".widget-pagerNext, .widget-pagerNext a")?;
        let mut entries = Vec::new();
        for work in state
            .values()
            .filter(|value| typename(value) == Some("Work"))
        {
            let Some(id) = string(work, "id") else {
                continue;
            };
            let Some(title) = string(work, "title") else {
                continue;
            };
            if title.is_empty() {
                continue;
            }
            let url = format!("{BASE_URL}/works/{id}");
            let mut item = CatalogItem::new(url.clone(), title);
            item.url = Some(url);
            item.cover = string(work, "adminCoverImageUrl")
                .or_else(|| string(work, "ogImageUrl"))
                .map(Into::into);
            item.language = Some("ja".into());
            entries.push(item);
        }
        Ok(Paged::new(entries, document.select(&next).next().is_some()))
    }

    fn parse_work(document: &Html, work_url: &str) -> Result<(CatalogItem, Vec<NovelChapter>)> {
        let state = Self::apollo_state(document)?;
        let parsed_work_url =
            Url::parse(work_url).map_err(|error| Error::new(error.to_string()))?;
        let work_id = parsed_work_url
            .path_segments()
            .and_then(|mut parts| parts.nth(1))
            .ok_or_else(|| Error::new("Kakuyomu work URL has no work id"))?
            .to_owned();
        let work = state
            .values()
            .find(|value| {
                typename(value) == Some("Work")
                    && string(value, "id").as_deref() == Some(work_id.as_str())
            })
            .ok_or_else(|| Error::new("Kakuyomu Apollo state has no requested work"))?;

        let author_ref = work
            .pointer("/author/__ref")
            .and_then(Value::as_str)
            .and_then(|value| value.strip_prefix("UserAccount:"));
        let author = author_ref.and_then(|id| {
            state.values().find(|value| {
                typename(value) == Some("UserAccount") && string(value, "id").as_deref() == Some(id)
            })
        });

        let title = require(string(work, "title"), "Kakuyomu work has no title")?;
        let mut item = CatalogItem::new(work_url, title);
        item.url = Some(work_url.to_owned());
        item.cover = string(work, "adminCoverImageUrl")
            .or_else(|| string(work, "ogImageUrl"))
            .map(Into::into);
        item.description = string(work, "introduction");
        item.authors = author
            .and_then(|value| string(value, "activityName"))
            .into_iter()
            .collect();
        item.tags = work
            .get("tagLabels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        item.status = Some(json!(match string(work, "serialStatus").as_deref() {
            Some("COMPLETED") => "completed",
            Some("RUNNING") => "ongoing",
            _ => "unknown",
        }));
        item.initialized = true;
        item.language = Some("ja".into());

        let mut chapters = Vec::new();
        for table in state
            .values()
            .filter(|value| typename(value) == Some("TableOfContentsChapter"))
        {
            let section = table
                .pointer("/chapter/__ref")
                .and_then(Value::as_str)
                .and_then(|reference| resolve_ref(&state, reference))
                .and_then(|chapter| string(chapter, "title"));
            let Some(episodes) = table.get("episodeUnions").and_then(Value::as_array) else {
                continue;
            };
            for episode in episodes {
                let Some(reference) = episode.get("__ref").and_then(Value::as_str) else {
                    continue;
                };
                let Some(episode) = resolve_ref(&state, reference) else {
                    continue;
                };
                let Some(id) = string(episode, "id") else {
                    continue;
                };
                let title = string(episode, "title").unwrap_or_default();
                let url = format!("{work_url}/episodes/{id}");
                chapters.push(NovelChapter {
                    key: url.clone(),
                    title: Some(title),
                    date_uploaded: string(episode, "publishedAt")
                        .and_then(|value| DateTime::parse_from_rfc3339(&value).ok())
                        .map(|value| value.timestamp_millis()),
                    url: Some(url),
                    language: Some("ja".into()),
                    source_order: Some(chapters.len() as i32),
                    section: section.clone(),
                    ..NovelChapter::default()
                });
            }
        }
        item.extra.insert("chapters".into(), json!(chapters));
        Ok((item, chapters))
    }

    fn parse_text(document: &Html, page_url: &str) -> Result<NovelText> {
        let chapter_title = text_for(document, ".chapterTitle")?;
        let episode_title = text_for(document, ".widget-episodeTitle")?;
        let body = html_for(document, ".widget-episodeBody")?
            .ok_or_else(|| Error::new("Kakuyomu episode has no readable body"))?;
        let title = episode_title.or(chapter_title.clone());
        let mut rendered = String::new();
        if let Some(section) = chapter_title.filter(|value| !value.is_empty()) {
            rendered.push_str("<h1>");
            rendered.push_str(&section);
            rendered.push_str("</h1>");
        }
        if let Some(episode) = title.as_ref().filter(|value| !value.is_empty()) {
            rendered.push_str("<h2>");
            rendered.push_str(episode);
            rendered.push_str("</h2>");
        }
        rendered.push_str(&body);
        Ok(NovelText {
            html: Some(rendered.clone()),
            title,
            base_url: Some(page_url.to_owned()),
            image_context: Some(image_context(page_url)),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }
}

impl NovelSource for KakuyomuSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing("popular", page, &json!({}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if listing != "popular" {
            return Err(Error::new(format!("unknown novel listing {listing:?}")));
        }
        let (document, _) = self.document(&Self::ranking_url(page.max(1), filters)?)?;
        Self::parse_listing(&document)
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/search"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        let (document, _) = self.document(url.as_str())?;
        Self::parse_search(&document)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item.url.as_deref().unwrap_or(&item.key);
        let (document, final_url) = self.document(url)?;
        Self::parse_work(&document, &final_url).map(|value| value.0)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        if let Some(chapters) = item
            .extra
            .get("chapters")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
        {
            return Ok(chapters);
        }
        let url = item.url.as_deref().unwrap_or(&item.key);
        let (document, final_url) = self.document(url)?;
        Self::parse_work(&document, &final_url).map(|value| value.1)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let (document, final_url) = self.document(url)?;
        Self::parse_text(&document, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter("genre", "Genre", KAKUYOMU_GENRES, 0),
            select_filter("period", "Period", KAKUYOMU_PERIODS, 0),
        ])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("kakuyomu.jp") {
            return Ok(None);
        }
        let parts = url
            .path_segments()
            .map(|parts| parts.collect::<Vec<_>>())
            .unwrap_or_default();
        if parts.first() != Some(&"works") || parts.get(1).is_none() {
            return Ok(None);
        }
        let work_url = format!("{BASE_URL}/works/{}", parts[1]);
        let mut result = UrlResolveResult {
            item: Some(CatalogItem::new(work_url.clone(), "")),
            ..UrlResolveResult::default()
        };
        if parts.get(2) == Some(&"episodes") && parts.get(3).is_some() {
            result.novel_chapter = Some(NovelChapter {
                key: candidate.to_owned(),
                url: Some(candidate.to_owned()),
                language: Some("ja".into()),
                ..NovelChapter::default()
            });
        }
        if let Some(item) = result.item.as_mut() {
            item.url = Some(work_url);
            item.language = Some("ja".into());
        }
        Ok(Some(result))
    }
}

fn typename(value: &Value) -> Option<&str> {
    value.get("__typename").and_then(Value::as_str)
}

fn string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn resolve_ref<'a>(state: &'a Map<String, Value>, reference: &str) -> Option<&'a Value> {
    state.get(reference)
}

fn filter(filters: &Value, key: &str, default: &str) -> String {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_owned()
}

fn text_for(document: &Html, query: &str) -> Result<Option<String>> {
    let selector = selector(query)?;
    Ok(document
        .select(&selector)
        .next()
        .map(html::text)
        .map(|value| normalize_space(&value))
        .filter(|value| !value.is_empty()))
}

fn html_for(document: &Html, query: &str) -> Result<Option<String>> {
    let selector = selector(query)?;
    Ok(document
        .select(&selector)
        .next()
        .map(|element| element.inner_html())
        .filter(|value| !value.trim().is_empty()))
}

fn image_context(referer: &str) -> ImageRequestContext {
    ImageRequestContext {
        headers: [
            ("Referer".to_owned(), referer.to_owned()),
            ("User-Agent".to_owned(), USER_AGENT.to_owned()),
        ]
        .into_iter()
        .collect(),
        cookie_url: None,
    }
}

fn select_filter(
    id: &str,
    name: &str,
    values: &[(&str, &str)],
    default_index: u32,
) -> FilterDefinition {
    FilterDefinition::Select {
        id: id.into(),
        name: name.into(),
        options: values
            .iter()
            .map(|(label, value)| OptionItem {
                label: (*label).into(),
                value: (*value).into(),
            })
            .collect(),
        default_index,
    }
}

const KAKUYOMU_GENRES: &[(&str, &str)] = &[
    ("総合", "all"),
    ("異世界ファンタジー", "fantasy"),
    ("現代ファンタジー", "action"),
    ("SF", "sf"),
    ("恋愛", "love_story"),
    ("ラブコメ", "romance"),
    ("現代ドラマ", "drama"),
    ("ホラー", "horror"),
    ("ミステリー", "mystery"),
    ("エッセイ・ノンフィクション", "nonfiction"),
    ("歴史・時代・伝奇", "history"),
    ("創作論・評論", "criticism"),
    ("詩・童話・その他", "others"),
];

const KAKUYOMU_PERIODS: &[(&str, &str)] = &[
    ("累計", "entire"),
    ("日間", "daily"),
    ("週間", "weekly"),
    ("月間", "monthly"),
    ("年間", "yearly"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, KakuyomuSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_filtered_ranking_urls() {
        let url = KakuyomuSource::ranking_url(2, &json!({"genre": "fantasy", "period": "weekly"}))
            .unwrap();
        assert_eq!(url, "https://kakuyomu.jp/rankings/fantasy/weekly?page=2");
    }

    #[test]
    fn parses_ranking_fixture() {
        let document = html::document(include_str!("../tests/fixtures/ranking.html"));
        let page = KakuyomuSource::parse_listing(&document).unwrap();
        assert_eq!(page.entries[0].title, "Fixture Work");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_work_and_chapter_fixture() {
        let document = html::document(include_str!("../tests/fixtures/work.html"));
        let (item, chapters) =
            KakuyomuSource::parse_work(&document, "https://kakuyomu.jp/works/123").unwrap();
        assert_eq!(item.title, "Fixture Work");
        assert_eq!(item.authors, ["Fixture Author"]);
        assert_eq!(chapters[0].title.as_deref(), Some("Opening"));
        assert_eq!(chapters[0].section.as_deref(), Some("Volume One"));
    }

    #[test]
    fn parses_episode_fixture() {
        let document = html::document(include_str!("../tests/fixtures/episode.html"));
        let text =
            KakuyomuSource::parse_text(&document, "https://kakuyomu.jp/works/123/episodes/episode")
                .unwrap();
        assert!(text.html.unwrap().contains("Fixture body."));
    }

    #[test]
    fn exposes_all_upstream_filters() {
        let filters = KakuyomuSource::default().filters().unwrap();
        assert_eq!(filters.len(), 2);
        match &filters[0] {
            FilterDefinition::Select { options, .. } => assert_eq!(options.len(), 13),
            other => panic!("unexpected filter: {other:?}"),
        }
    }
}
