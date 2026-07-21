// Adapted from LNReader/lnreader-plugins under the MIT license.

use chrono::{NaiveDate, NaiveDateTime};
use manatan_common::{absolute_url, attr, normalize_space, require, selector};
use manatan_sdk::{
    client::Client,
    html::{self, Html},
    model::{
        CatalogItem, FilterDefinition, ImageRequestContext, NovelChapter, NovelChapterPage,
        NovelContentBlock, NovelText, OptionItem, Paged, UrlResolveResult,
    },
    Error, NovelSource, Result,
};
use serde_json::{json, Value};
use url::Url;

#[cfg(target_arch = "wasm32")]
const SOURCE_ID: &str = "yomou.syosetu";
const RANKING_URL: &str = "https://yomou.syosetu.com";
const NOVEL_URL: &str = "https://ncode.syosetu.com";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

pub struct SyosetuSource {
    client: Client,
}

impl Default for SyosetuSource {
    fn default() -> Self {
        Self {
            client: Client::new().header("User-Agent", USER_AGENT),
        }
    }
}

impl SyosetuSource {
    fn document(&self, url: &str) -> Result<(Html, String)> {
        let response = self.client.get(url).send()?.error_for_status()?;
        Ok((
            html::document(response.text()?),
            response.final_url().to_owned(),
        ))
    }

    fn ranking_url(page: u32, filters: &Value) -> String {
        let ranking = filter(filters, "ranking", "total");
        let genre = filter(filters, "genre", "");
        let modifier = filter(filters, "modifier", "total");
        let path = if genre.is_empty() {
            format!("rank/list/type/{ranking}_{modifier}")
        } else {
            let family = if genre.len() == 1 {
                "isekailist"
            } else {
                "genrelist"
            };
            let suffix = if modifier != "total" {
                format!("_{modifier}")
            } else {
                String::new()
            };
            format!("rank/{family}/type/{ranking}_{genre}{suffix}")
        };
        format!("{RANKING_URL}/{path}/?p={}", page.clamp(1, 100))
    }

    fn parse_ranking(document: &Html, requested_page: u32) -> Result<Paged<CatalogItem>> {
        let current = text_for(document, ".c-pager__item.is-current")?
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        if current != requested_page.clamp(1, 100) {
            return Ok(Paged::default());
        }
        let items = parse_items(document, ".c-card", ".p-ranklist-item__title a")?;
        Ok(Paged::new(items, current < last_page(document)?))
    }

    fn search_url(query: &str, page: u32) -> Result<String> {
        let mut url = Url::parse(&format!("{RANKING_URL}/search.php"))
            .map_err(|error| Error::new(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("order", "hyoka")
            .append_pair("p", &page.clamp(1, 100).to_string())
            .append_pair("word", query);
        Ok(url.to_string())
    }

    fn parse_search(document: &Html) -> Result<Paged<CatalogItem>> {
        let items = parse_items(document, ".searchkekka_box", ".novel_h a")?;
        let current = text_for(document, ".c-pager__item.is-current")?
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        Ok(Paged::new(items, current < last_page(document)?))
    }

    fn parse_details(document: &Html, page_url: &str) -> Result<CatalogItem> {
        let title = require(
            text_for(document, ".p-novel__title")?,
            "Syosetu work has no title",
        )?;
        let author = text_for(document, ".p-novel__author")?
            .map(|value| value.trim_start_matches("作者：").trim().to_owned())
            .filter(|value| !value.is_empty());
        let announcement = text_for(document, ".c-announce")?.unwrap_or_default();
        let status = if announcement.contains("更新されていません") {
            "hiatus"
        } else if announcement.contains("連載中") || announcement.contains("未完結") {
            "ongoing"
        } else if announcement.contains("完結") {
            "completed"
        } else {
            "unknown"
        };
        let description = html_for(document, "#novel_ex")?;
        let meta = selector("meta[property=\"og:description\"]")?;
        let tags = document
            .select(&meta)
            .next()
            .and_then(|element| attr(element, "content"))
            .map(|value| {
                value
                    .split_whitespace()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let mut item = CatalogItem::new(page_url, title);
        item.url = Some(page_url.to_owned());
        item.description = description;
        item.authors = author.into_iter().collect();
        item.tags = tags;
        item.status = Some(json!(status));
        item.initialized = true;
        item.language = Some("ja".into());
        item.content_rating = Some("safe".into());
        Ok(item)
    }

    fn parse_chapter_page(document: &Html, page: u32) -> Result<NovelChapterPage> {
        let rows = selector(".p-eplist__sublist")?;
        let link = selector("a")?;
        let updated = selector(".p-eplist__update")?;
        let mut entries = Vec::new();
        for row in document.select(&rows) {
            let Some(anchor) = row.select(&link).next() else {
                continue;
            };
            let Some(href) = attr(anchor, "href") else {
                continue;
            };
            let title = normalize_space(&html::text(anchor));
            if title.is_empty() {
                continue;
            }
            let url = absolute_url(NOVEL_URL, &href)?;
            let date = row
                .select(&updated)
                .next()
                .map(html::text)
                .and_then(|value| parse_date(&value));
            entries.push(NovelChapter {
                key: url.clone(),
                title: Some(title),
                chapter_number: chapter_number_from_url(&url),
                date_uploaded: date,
                url: Some(url),
                language: Some("ja".into()),
                source_order: Some(entries.len() as i32),
                page: Some(page),
                ..NovelChapter::default()
            });
        }
        let count = last_page(document)?;
        Ok(NovelChapterPage {
            entries,
            has_next_page: page < count,
            page_count: Some(count),
        })
    }

    fn parse_text(document: &Html, page_url: &str) -> Result<NovelText> {
        let title = text_for(document, ".p-novel__title")?;
        let body = html_for(
            document,
            ".p-novel__body .p-novel__text:not([class*=\"p-novel__text--\"])",
        )?
        .ok_or_else(|| Error::new("Syosetu chapter has no readable body"))?;
        let mut rendered = String::new();
        if let Some(title) = title.as_ref().filter(|value| !value.is_empty()) {
            rendered.push_str("<h1>");
            rendered.push_str(title);
            rendered.push_str("</h1>");
        }
        rendered.push_str(&body);
        Ok(NovelText {
            html: Some(rendered.clone()),
            title,
            base_url: Some(page_url.to_owned()),
            image_context: Some(ImageRequestContext {
                headers: [
                    ("Referer".to_owned(), page_url.to_owned()),
                    ("User-Agent".to_owned(), USER_AGENT.to_owned()),
                ]
                .into_iter()
                .collect(),
                cookie_url: None,
            }),
            blocks: vec![NovelContentBlock::Text {
                text: rendered,
                html: true,
            }],
            ..NovelText::default()
        })
    }

    fn work_url(item: &CatalogItem) -> Result<String> {
        let candidate = item.url.as_deref().unwrap_or(&item.key);
        let mut url = Url::parse(&absolute_url(NOVEL_URL, candidate)?)
            .map_err(|error| Error::new(error.to_string()))?;
        let code = url
            .path_segments()
            .and_then(|mut parts| parts.next().map(str::to_owned))
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("Syosetu URL has no novel code"))?;
        url.set_path(&format!("/{code}/"));
        url.set_query(None);
        url.set_fragment(None);
        Ok(url.to_string())
    }
}

impl NovelSource for SyosetuSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.listing("popular", page, &json!({}))
    }

    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if listing != "popular" {
            return Err(Error::new(format!("unknown novel listing {listing:?}")));
        }
        let requested_page = page.clamp(1, 100);
        let (document, _) = self.document(&Self::ranking_url(requested_page, filters))?;
        Self::parse_ranking(&document, requested_page)
    }

    fn search(&mut self, query: &str, page: u32, _filters: &Value) -> Result<Paged<CatalogItem>> {
        let (document, _) = self.document(&Self::search_url(query, page)?)?;
        Self::parse_search(&document)
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let url = Self::work_url(&item)?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_details(&document, &final_url)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<NovelChapter>> {
        let url = Self::work_url(&item)?;
        let (first, final_url) = self.document(&url)?;
        let first_page = Self::parse_chapter_page(&first, 1)?;
        let page_count = first_page.page_count.unwrap_or(1);
        let mut entries = first_page.entries;
        for page in 2..=page_count {
            let page_url = format!("{final_url}?p={page}");
            let (document, _) = self.document(&page_url)?;
            entries.extend(Self::parse_chapter_page(&document, page)?.entries);
        }
        for (index, chapter) in entries.iter_mut().enumerate() {
            chapter.source_order = Some(index as i32);
        }
        Ok(entries)
    }

    fn chapters_page(&mut self, item: CatalogItem, page: u32) -> Result<NovelChapterPage> {
        let work_url = Self::work_url(&item)?;
        let page = page.max(1);
        let url = if page == 1 {
            work_url
        } else {
            format!("{work_url}?p={page}")
        };
        let (document, _) = self.document(&url)?;
        Self::parse_chapter_page(&document, page)
    }

    fn text(&mut self, _item: CatalogItem, chapter: NovelChapter) -> Result<NovelText> {
        let url = absolute_url(NOVEL_URL, chapter.url.as_deref().unwrap_or(&chapter.key))?;
        let (document, final_url) = self.document(&url)?;
        Self::parse_text(&document, &final_url)
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![
            select_filter("ranking", "Ranked by", RANKING, 5),
            select_filter("genre", "Ranking Genre", GENRES, 0),
            select_filter("modifier", "Modifier", MODIFIERS, 0),
        ])
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let url = Url::parse(candidate).map_err(|error| Error::new(error.to_string()))?;
        if url.host_str() != Some("ncode.syosetu.com") {
            return Ok(None);
        }
        let parts = url
            .path_segments()
            .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
            .unwrap_or_default();
        let Some(code) = parts.first() else {
            return Ok(None);
        };
        let work_url = format!("{NOVEL_URL}/{code}/");
        let mut item = CatalogItem::new(work_url.clone(), "");
        item.url = Some(work_url);
        item.language = Some("ja".into());
        item.content_rating = Some("safe".into());
        let novel_chapter = (parts.len() > 1).then(|| NovelChapter {
            key: candidate.to_owned(),
            url: Some(candidate.to_owned()),
            chapter_number: parts.get(1).and_then(|value| value.parse().ok()),
            language: Some("ja".into()),
            ..NovelChapter::default()
        });
        Ok(Some(UrlResolveResult {
            item: Some(item),
            novel_chapter,
            ..UrlResolveResult::default()
        }))
    }
}

fn parse_items(document: &Html, row_query: &str, link_query: &str) -> Result<Vec<CatalogItem>> {
    let rows = selector(row_query)?;
    let links = selector(link_query)?;
    let mut entries = Vec::new();
    for row in document.select(&rows) {
        let Some(anchor) = row.select(&links).next() else {
            continue;
        };
        let Some(href) = attr(anchor, "href") else {
            continue;
        };
        let title = normalize_space(&html::text(anchor));
        if title.is_empty() {
            continue;
        }
        let url = absolute_url(NOVEL_URL, &href)?;
        let mut item = CatalogItem::new(url.clone(), title);
        item.url = Some(url);
        item.language = Some("ja".into());
        item.content_rating = Some("safe".into());
        entries.push(item);
    }
    Ok(entries)
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

fn last_page(document: &Html) -> Result<u32> {
    let last = selector(".c-pager__item--last")?;
    let page = document
        .select(&last)
        .next()
        .and_then(|element| attr(element, "href"))
        .and_then(|href| Url::parse(&absolute_url(RANKING_URL, &href).ok()?).ok())
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "p")
                .and_then(|(_, value)| value.parse().ok())
        });
    if let Some(page) = page {
        return Ok(page);
    }
    let current = text_for(document, ".c-pager__item.is-current")?
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let links = selector(".c-pager__item[href]")?;
    Ok(document
        .select(&links)
        .filter_map(|element| attr(element, "href"))
        .filter_map(|href| Url::parse(&absolute_url(RANKING_URL, &href).ok()?).ok())
        .filter_map(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "p")
                .and_then(|(_, value)| value.parse::<u32>().ok())
        })
        .max()
        .unwrap_or(current))
}

fn parse_date(value: &str) -> Option<i64> {
    let value = value.trim();
    NaiveDateTime::parse_from_str(value, "%Y/%m/%d %H:%M")
        .ok()
        .map(|value| value.and_utc().timestamp_millis())
        .or_else(|| {
            NaiveDate::parse_from_str(value.split_whitespace().next()?, "%Y/%m/%d")
                .ok()
                .and_then(|value| value.and_hms_opt(0, 0, 0))
                .map(|value| value.and_utc().timestamp_millis())
        })
}

fn chapter_number_from_url(value: &str) -> Option<f32> {
    Url::parse(value)
        .ok()?
        .path_segments()?
        .filter(|part| !part.is_empty())
        .nth(1)?
        .parse()
        .ok()
}

fn filter(filters: &Value, key: &str, default: &str) -> String {
    filters
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_owned()
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

const RANKING: &[(&str, &str)] = &[
    ("日間", "daily"),
    ("週間", "weekly"),
    ("月間", "monthly"),
    ("四半期", "quarter"),
    ("年間", "yearly"),
    ("累計", "total"),
];

const GENRES: &[(&str, &str)] = &[
    ("総ジャンル", ""),
    ("異世界転生/転移〔恋愛〕", "1"),
    ("異世界転生/転移〔ファンタジー〕", "2"),
    ("異世界転生/転移〔文芸・SF・その他〕", "o"),
    ("異世界〔恋愛〕", "101"),
    ("現実世界〔恋愛〕", "102"),
    ("ハイファンタジー〔ファンタジー〕", "201"),
    ("ローファンタジー〔ファンタジー〕", "202"),
    ("純文学〔文芸〕", "301"),
    ("ヒューマンドラマ〔文芸〕", "302"),
    ("歴史〔文芸〕", "303"),
    ("推理〔文芸〕", "304"),
    ("ホラー〔文芸〕", "305"),
    ("アクション〔文芸〕", "306"),
    ("コメディー〔文芸〕", "307"),
    ("VRゲーム〔SF〕", "401"),
    ("宇宙〔SF〕", "402"),
    ("空想科学〔SF〕", "403"),
    ("パニック〔SF〕", "404"),
    ("童話〔その他〕", "9901"),
    ("詩〔その他〕", "9902"),
    ("エッセイ〔その他〕", "9903"),
    ("その他〔その他〕", "9999"),
];

const MODIFIERS: &[(&str, &str)] = &[
    ("すべて", "total"),
    ("連載中", "r"),
    ("完結済", "er"),
    ("短編", "t"),
];

#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().novel(SOURCE_ID, SyosetuSource::default())
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_every_ranking_url_shape() {
        assert_eq!(
            SyosetuSource::ranking_url(2, &json!({})),
            "https://yomou.syosetu.com/rank/list/type/total_total/?p=2"
        );
        assert_eq!(
            SyosetuSource::ranking_url(
                3,
                &json!({"ranking":"weekly", "genre":"201", "modifier":"r"}),
            ),
            "https://yomou.syosetu.com/rank/genrelist/type/weekly_201_r/?p=3"
        );
    }

    #[test]
    fn parses_ranking_fixture() {
        let document = html::document(include_str!("../tests/fixtures/ranking.html"));
        let page = SyosetuSource::parse_ranking(&document, 1).unwrap();
        assert_eq!(page.entries[0].title, "Fixture Novel");
        assert!(page.has_next_page);
    }

    #[test]
    fn parses_details_and_chapters_fixture() {
        let document = html::document(include_str!("../tests/fixtures/work.html"));
        let item =
            SyosetuSource::parse_details(&document, "https://ncode.syosetu.com/n1234ab/").unwrap();
        assert_eq!(item.title, "Fixture Novel");
        assert_eq!(item.status, Some(json!("completed")));
        let page = SyosetuSource::parse_chapter_page(&document, 1).unwrap();
        assert_eq!(page.entries[0].title.as_deref(), Some("Chapter One"));
        assert_eq!(page.page_count, Some(2));
    }

    #[test]
    fn parses_episode_fixture() {
        let document = html::document(include_str!("../tests/fixtures/episode.html"));
        let text =
            SyosetuSource::parse_text(&document, "https://ncode.syosetu.com/n1234ab/1/").unwrap();
        assert!(text.html.unwrap().contains("Fixture body."));
    }

    #[test]
    fn exposes_all_upstream_filters() {
        let filters = SyosetuSource::default().filters().unwrap();
        assert_eq!(filters.len(), 3);
        match &filters[1] {
            FilterDefinition::Select { options, .. } => assert_eq!(options.len(), 23),
            other => panic!("unexpected filter: {other:?}"),
        }
    }
}
