// Adapted from LNReader/lnreader-plugins under the MIT license.

use chrono::DateTime;
use manatan_common::{normalize_space, require, selector};
use manatan_sdk::{
    client::{Client, Response},
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequestContext, NovelChapter, NovelContentBlock,
        NovelText, OptionItem, Paged, UrlResolveResult,
    },
    services, Error, NovelSource, Result,
};
use serde_json::{json, Map, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "kakuyomu";
const BASE_URL: &str = "https://kakuyomu.jp";
const HTML_SELECT_SERVICE: &str = "html.select.v1";
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
    fn response(&self, url: &str) -> Result<Response> {
        self.client.get(url).send()?.error_for_status()
    }

    fn select(&self, response: &Response, queries: Value) -> Result<Option<Value>> {
        if !services::is_available(HTML_SELECT_SERVICE) {
            return Ok(None);
        }
        let request = json!({
            "fragment": false,
            "queries": queries,
        });
        services::invoke_binary(HTML_SELECT_SERVICE, &request, response.bytes())
            .map(|(response, _): (Value, Vec<u8>)| Some(response))
    }

    fn ranking_url(page: u32, filters: &Value) -> Result<String> {
        let genre = filter(filters, "genre", "all");
        let period = filter(filters, "period", "entire");
        let mut url = Url::parse(&format!("{BASE_URL}/rankings/{genre}/{period}"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut().append_pair("work_variation", "long");
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        Ok(url.to_string())
    }

    fn preview_cover_url(work_url: &str) -> Option<String> {
        let url = Url::parse(work_url).ok()?;
        let mut segments = url.path_segments()?;
        if segments.next()? != "works" {
            return None;
        }
        let work_id = segments.next()?.trim();
        if work_id.is_empty() {
            return None;
        }
        Some(format!(
            "https://cdn-static.kakuyomu.jp/works/{work_id}/ogimage.png"
        ))
    }

    fn apollo_state(document: &Html) -> Result<Map<String, Value>> {
        let data = selector("script#__NEXT_DATA__[type=\"application/json\"]")?;
        let json = document
            .select(&data)
            .next()
            .map(|element| element.inner_html())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Error::new("Kakuyomu page has no __NEXT_DATA__ payload"))?;
        Self::apollo_state_json(&json)
    }

    fn apollo_state_json(json: &str) -> Result<Map<String, Value>> {
        let root: Value = serde_json::from_str(json)?;
        root.pointer("/props/pageProps/__APOLLO_STATE__")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| Error::new("Kakuyomu page has no Apollo state"))
    }

    fn parse_ranking(document: &Html) -> Result<Paged<CatalogItem>> {
        let state = Self::apollo_state(document)?;
        Self::parse_connection_state(&state, "rankedWorks(")
    }

    fn parse_search(document: &Html) -> Result<Paged<CatalogItem>> {
        let state = Self::apollo_state(document)?;
        Self::parse_connection_state(&state, "searchWorks(")
    }

    fn parse_connection_state(
        state: &Map<String, Value>,
        field_prefix: &str,
    ) -> Result<Paged<CatalogItem>> {
        let connection = state
            .get("ROOT_QUERY")
            .and_then(Value::as_object)
            .and_then(|query| {
                query
                    .iter()
                    .find(|(key, _)| key.starts_with(field_prefix))
                    .map(|(_, value)| value)
            })
            .ok_or_else(|| Error::new(format!("Kakuyomu page has no {field_prefix} connection")))?;
        let nodes = connection
            .get("nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                Error::new(format!("Kakuyomu {field_prefix} connection has no nodes"))
            })?;
        let mut entries = Vec::with_capacity(nodes.len());
        for node in nodes {
            let Some(reference) = node.get("__ref").and_then(Value::as_str) else {
                continue;
            };
            let Some(work) = resolve_ref(state, reference) else {
                continue;
            };
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
                .or_else(|| Self::preview_cover_url(&item.key))
                .map(Into::into);
            item.language = Some("ja".into());
            item.content_rating = Some("safe".into());
            entries.push(item);
        }
        let has_next_page = connection
            .pointer("/pageInfo/hasNextPage")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(Paged::new(entries, has_next_page))
    }

    fn parse_work(document: &Html, work_url: &str) -> Result<(CatalogItem, Vec<NovelChapter>)> {
        let state = Self::apollo_state(document)?;
        Self::parse_work_state(state, work_url)
    }

    fn parse_work_state(
        state: Map<String, Value>,
        work_url: &str,
    ) -> Result<(CatalogItem, Vec<NovelChapter>)> {
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
        item.content_rating = Some("safe".into());

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
        Self::parse_text_parts(chapter_title, episode_title, body, page_url)
    }

    fn parse_text_parts(
        chapter_title: Option<String>,
        episode_title: Option<String>,
        body: String,
        page_url: &str,
    ) -> Result<NovelText> {
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
        let response = self.response(&Self::ranking_url(page.max(1), filters)?)?;
        if let Some(selection) = self.select(&response, apollo_queries())? {
            let state = Self::apollo_state_json(selected_apollo_json(&selection)?)?;
            return Self::parse_connection_state(&state, "rankedWorks(");
        }
        Self::parse_ranking(&html::document(response.text()?))
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let mut url = Url::parse(&format!("{BASE_URL}/search"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut().append_pair("q", query);
        if page > 1 {
            url.query_pairs_mut().append_pair("page", &page.to_string());
        }
        let response = self.response(url.as_str())?;
        if let Some(selection) = self.select(&response, apollo_queries())? {
            let state = Self::apollo_state_json(selected_apollo_json(&selection)?)?;
            return Self::parse_connection_state(&state, "searchWorks(");
        }
        Self::parse_search(&html::document(response.text()?))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = item.url.as_deref().unwrap_or(&item.key);
        let response = self.response(url)?;
        let final_url = response.final_url().to_owned();
        if let Some(selection) = self.select(&response, apollo_queries())? {
            let state = Self::apollo_state_json(selected_apollo_json(&selection)?)?;
            return Self::parse_work_state(state, &final_url).map(|value| value.0);
        }
        Self::parse_work(&html::document(response.text()?), &final_url).map(|value| value.0)
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
        let response = self.response(url)?;
        let final_url = response.final_url().to_owned();
        if let Some(selection) = self.select(&response, apollo_queries())? {
            let state = Self::apollo_state_json(selected_apollo_json(&selection)?)?;
            return Self::parse_work_state(state, &final_url).map(|value| value.1);
        }
        Self::parse_work(&html::document(response.text()?), &final_url).map(|value| value.1)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = chapter.url.as_deref().unwrap_or(&chapter.key);
        let response = self.response(url)?;
        let final_url = response.final_url().to_owned();
        if let Some(selection) = self.select(
            &response,
            json!([
                {
                    "id": "chapterTitle",
                    "selector": ".chapterTitle",
                    "limit": 1,
                    "fields": [{ "name": "value", "value": { "type": "text" } }]
                },
                {
                    "id": "episodeTitle",
                    "selector": ".widget-episodeTitle",
                    "limit": 1,
                    "fields": [{ "name": "value", "value": { "type": "text" } }]
                },
                {
                    "id": "body",
                    "selector": ".widget-episodeBody",
                    "limit": 1,
                    "fields": [{ "name": "value", "value": { "type": "innerHtml" } }]
                }
            ]),
        )? {
            let chapter_title = selected_first_string(&selection, "chapterTitle", "value")
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            let episode_title = selected_first_string(&selection, "episodeTitle", "value")
                .map(|value| normalize_space(&value))
                .filter(|value| !value.is_empty());
            let body = selected_first_string(&selection, "body", "value")
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| Error::new("Kakuyomu episode has no readable body"))?;
            return Self::parse_text_parts(chapter_title, episode_title, body, &final_url);
        }
        Self::parse_text(&html::document(response.text()?), &final_url)
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
            item.content_rating = Some("safe".into());
        }
        Ok(Some(result))
    }
}

fn apollo_queries() -> Value {
    json!([{
        "id": "apollo",
        "selector": "script#__NEXT_DATA__[type=\"application/json\"]",
        "limit": 1,
        "fields": [{ "name": "json", "value": { "type": "innerHtml" } }]
    }])
}

fn selected_matches<'a>(selection: &'a Value, id: &str) -> Result<&'a [Value]> {
    selection
        .get("results")
        .and_then(|results| results.get(id))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| Error::new(format!("HTML selection has no result {id:?}")))
}

fn selected_string(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn selected_first_string(selection: &Value, id: &str, field: &str) -> Option<String> {
    selected_matches(selection, id)
        .ok()
        .and_then(|matches| matches.first())
        .and_then(|value| selected_string(value, field))
}

fn selected_apollo_json(selection: &Value) -> Result<&str> {
    selected_matches(selection, "apollo")?
        .first()
        .and_then(|value| value.get("json"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Error::new("Kakuyomu page has no __NEXT_DATA__ payload"))
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
        assert_eq!(
            url,
            "https://kakuyomu.jp/rankings/fantasy/weekly?work_variation=long&page=2"
        );
    }

    #[test]
    fn parses_ranking_fixture() {
        let document = html::document(include_str!("../tests/fixtures/ranking.html"));
        let page = KakuyomuSource::parse_ranking(&document).unwrap();
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].title, "First Ranked Work");
        assert_eq!(page.entries[1].title, "Fixture Work");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|request| request.url.as_str()),
            Some("https://cdn-static.kakuyomu.jp/custom-cover.png")
        );
        assert_eq!(
            page.entries[1]
                .cover
                .as_ref()
                .map(|request| request.url.as_str()),
            Some("https://cdn-static.kakuyomu.jp/works/123/ogimage.png")
        );
        assert!(page.has_next_page);
    }

    #[test]
    fn derives_preview_covers_only_for_work_urls() {
        assert_eq!(
            KakuyomuSource::preview_cover_url("https://kakuyomu.jp/works/123/episodes/456")
                .as_deref(),
            Some("https://cdn-static.kakuyomu.jp/works/123/ogimage.png")
        );
        assert!(KakuyomuSource::preview_cover_url("https://kakuyomu.jp/rankings/all").is_none());
        assert!(KakuyomuSource::preview_cover_url("not a URL").is_none());
    }

    #[test]
    fn parses_search_connection_order_and_pagination() {
        let state = KakuyomuSource::apollo_state_json(
            r#"{"props":{"pageProps":{"__APOLLO_STATE__":{"ROOT_QUERY":{"searchWorks({\"first\":20})":{"nodes":[{"__ref":"Work:2"},{"__ref":"Work:1"}],"pageInfo":{"hasNextPage":true}}},"Work:1":{"__typename":"Work","id":"1","title":"Second Result"},"Work:2":{"__typename":"Work","id":"2","title":"First Result"}}}}}"#,
        )
        .unwrap();
        let page = KakuyomuSource::parse_connection_state(&state, "searchWorks(").unwrap();
        assert_eq!(
            page.entries
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["First Result", "Second Result"]
        );
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
