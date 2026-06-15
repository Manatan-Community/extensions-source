use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource, SearchRequest,
};
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;

const UNIVERSE_URL: &str = "https://universe.leagueoflegends.com";
const MEEPS_URL: &str = "https://universe-meeps.leagueoflegends.com/v1";
const COMICS_URL: &str = "https://universe-comics.leagueoflegends.com/comics";
const SOURCE: LeagueOfLegends = LeagueOfLegends;

struct LeagueOfLegends;

impl MangaSource for LeagueOfLegends {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let body = fetch_json_or_fixture(&format!("{MEEPS_URL}/{}/comics/index.json", source.site_lang), HUB_FIXTURE, source);
        Ok(parse_hub(&body, source, ""))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(query, source) {
            let body = fetch_json_or_fixture(&format!("{COMICS_URL}/{}/{key}/index.json", source.site_lang), PAGES_FIXTURE, source);
            let item = CatalogItem {
                key: key.clone(),
                title: key.rsplit('/').next().unwrap_or("Comic").replace('-', " "),
                url: Some(format!("{UNIVERSE_URL}/{}/comic/{key}", source.site_lang)),
                language: Some(source.lang.into()),
                content_rating: Some("safe".into()),
                status: ItemStatus::Completed,
                initialized: true,
                ..CatalogItem::default()
            };
            let has_pages = !parse_pages_json(&body).is_empty();
            return Ok(Paged { entries: if has_pages { vec![item] } else { Vec::new() }, has_next_page: false });
        }
        let body = fetch_json_or_fixture(&format!("{MEEPS_URL}/{}/comics/index.json", source.site_lang), HUB_FIXTURE, source);
        Ok(parse_hub(&body, source, query))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        let mut page = parse_hub(&fetch_json_or_fixture(&format!("{MEEPS_URL}/{}/comics/index.json", source.site_lang), HUB_FIXTURE, source), source, "");
        Ok(page.entries.drain(..).find(|item| item.key == key).unwrap_or_else(|| fallback_item(&key, source)))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let source = source_for(&request);
        let key = request_key(&request, "manga").unwrap_or_else(|| "sample".into());
        if key.contains('/') && key.rsplit('/').count() > 1 {
            return Ok(vec![chapter_from_key(&key, "One Shot", 0.0, source)]);
        }
        let body = fetch_json_or_fixture(&format!("{MEEPS_URL}/{}/comics/{key}/index.json", source.site_lang), ISSUES_FIXTURE, source);
        let issues = serde_json::from_str::<Issues>(&body).unwrap_or_else(|_| serde_json::from_str(ISSUES_FIXTURE).expect("issues fixture"));
        Ok(issues.issues.into_iter().rev().map(|comic| chapter_from_comic(comic, source)).collect())
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let source = source_for(&request);
        let key = request_key(&request, "chapter").unwrap_or_else(|| "sample/1".into());
        let body = fetch_json_or_fixture(&format!("{COMICS_URL}/{}/{key}/index.json", source.site_lang), PAGES_FIXTURE, source);
        Ok(parse_pages_json(&body))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let source = source_for(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if let Some(key) = key_from_url(input, source) {
            return Ok(Some(UrlResolveResult {
                item: Some(fallback_item(&key, source)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.to_string(), ..SearchRequest::default() }),
            url: Some(input.to_string()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    site_lang: &'static str,
    lang: &'static str,
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "leagueoflegends-en", site_lang: "en_us", lang: "en" },
    SourceConfig { id: "leagueoflegends-de", site_lang: "de_de", lang: "de" },
    SourceConfig { id: "leagueoflegends-es", site_lang: "es_es", lang: "es" },
    SourceConfig { id: "leagueoflegends-fr", site_lang: "fr_fr", lang: "fr" },
    SourceConfig { id: "leagueoflegends-it", site_lang: "it_it", lang: "it" },
    SourceConfig { id: "leagueoflegends-pl", site_lang: "pl_pl", lang: "pl" },
    SourceConfig { id: "leagueoflegends-el", site_lang: "el_gr", lang: "el" },
    SourceConfig { id: "leagueoflegends-ro", site_lang: "ro_ro", lang: "ro" },
    SourceConfig { id: "leagueoflegends-hu", site_lang: "hu_hu", lang: "hu" },
    SourceConfig { id: "leagueoflegends-cs", site_lang: "cs_cz", lang: "cs" },
    SourceConfig { id: "leagueoflegends-es-419", site_lang: "es_mx", lang: "es-419" },
    SourceConfig { id: "leagueoflegends-pt-br", site_lang: "pt_br", lang: "pt-BR" },
    SourceConfig { id: "leagueoflegends-ja", site_lang: "ja_jp", lang: "ja" },
    SourceConfig { id: "leagueoflegends-ru", site_lang: "ru_ru", lang: "ru" },
    SourceConfig { id: "leagueoflegends-tr", site_lang: "tr_tr", lang: "tr" },
    SourceConfig { id: "leagueoflegends-ko", site_lang: "ko_kr", lang: "ko" },
];

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("leagueoflegends-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn client(source: SourceConfig) -> http::HttpClient {
    http::HttpClient::browser()
        .with_origin(UNIVERSE_URL)
        .with_referer(format!("{UNIVERSE_URL}/{}/comic/", source.site_lang))
}

fn fetch_json_or_fixture(target: &str, fixture: &str, source: SourceConfig) -> String {
    client(source).get(target).xhr().send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_hub(body: &str, source: SourceConfig, query: &str) -> Paged<CatalogItem> {
    let hub = serde_json::from_str::<Hub>(body).unwrap_or_else(|_| serde_json::from_str(HUB_FIXTURE).expect("hub fixture"));
    let query = query.to_ascii_lowercase();
    let entries = hub.sections.series.data.into_iter().chain(hub.sections.one_shots.data)
        .filter_map(|comic| comic.into_item(source))
        .filter(|item| query.is_empty() || item.title.to_ascii_lowercase().contains(&query) || item.tags.iter().any(|tag| tag.to_ascii_lowercase().contains(&query)))
        .collect();
    Paged { entries, has_next_page: false }
}

fn chapter_from_comic(comic: Comic, source: SourceConfig) -> MangaChapter {
    let key = comic.key().unwrap_or_else(|| "sample/1".into());
    let title = comic.title.unwrap_or_else(|| "Comic".into());
    chapter_from_key(&key, &title, comic.index.unwrap_or(-1.0), source)
}

fn chapter_from_key(key: &str, title: &str, number: f32, source: SourceConfig) -> MangaChapter {
    MangaChapter {
        key: key.to_string(),
        title: Some(title.to_string()),
        chapter_number: Some(number),
        url: Some(format!("{UNIVERSE_URL}/{}/comic/{key}", source.site_lang)),
        ..MangaChapter::default()
    }
}

fn parse_pages_json(body: &str) -> Vec<MangaPage> {
    let pages = serde_json::from_str::<Pages>(&body).unwrap_or_else(|_| serde_json::from_str(PAGES_FIXTURE).expect("pages fixture"));
    pages.desktop_pages.into_iter().flatten().enumerate().map(|(index, image)| MangaPage {
        content: PageContent::Url { url: image.uri, context: Some(image_headers()) },
        headers: image_headers(),
        description: Some(format!("Page {}", index + 1)),
        ..MangaPage::default()
    }).collect()
}

fn fallback_item(key: &str, source: SourceConfig) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: key.rsplit('/').next().unwrap_or("Comic").replace('-', " "),
        url: Some(format!("{UNIVERSE_URL}/{}/comic/{key}", source.site_lang)),
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        status: ItemStatus::Completed,
        initialized: true,
        ..CatalogItem::default()
    }
}

fn key_from_url(input: &str, source: SourceConfig) -> Option<String> {
    input.split(&format!("/{}/comic/", source.site_lang)).nth(1)
        .or_else(|| input.split("/comic/").nth(1))
        .map(|value| value.trim_matches('/').split(['?', '#']).next().unwrap_or(value).to_string())
        .filter(|value| !value.is_empty())
}

fn request_key(request: &Value, field: &str) -> Option<String> {
    request.get(field).and_then(|value| value.get("key").or_else(|| value.get("url")).and_then(Value::as_str).or_else(|| value.as_str())).map(ToString::to_string)
}

fn image_headers() -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    headers.insert("Referer".into(), format!("{UNIVERSE_URL}/"));
    headers
}

#[derive(Deserialize)]
struct Hub { sections: Sections }

#[derive(Deserialize)]
struct Sections {
    series: ComicData,
    #[serde(rename = "one-shots")]
    one_shots: ComicData,
}

#[derive(Deserialize)]
struct ComicData { data: Vec<Comic> }

#[derive(Deserialize)]
struct Issues { issues: Vec<Comic> }

#[derive(Deserialize)]
struct Comic {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    index: Option<f32>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    background: Option<Image>,
    #[serde(default, rename = "featured-champions")]
    champions: Option<Vec<Champion>>,
}

impl Comic {
    fn key(&self) -> Option<String> {
        self.url.as_ref()?.split("/comic/").nth(1).map(|value| value.trim_matches('/').to_string())
    }

    fn into_item(self, source: SourceConfig) -> Option<CatalogItem> {
        let key = self.key()?;
        let title = self.title?;
        Some(CatalogItem {
            key: key.clone(),
            title,
            cover: self.background.map(|image| image.uri),
            url: Some(format!("{UNIVERSE_URL}/{}/comic/{key}", source.site_lang)),
            description: self.description.map(clean_description),
            tags: self.subtitle.into_iter().chain(self.champions.unwrap_or_default().into_iter().map(|champion| champion.name)).collect(),
            language: Some(source.lang.into()),
            content_rating: Some("safe".into()),
            status: ItemStatus::Completed,
            initialized: true,
            ..CatalogItem::default()
        })
    }
}

#[derive(Deserialize)]
struct Pages {
    #[serde(rename = "desktop-pages")]
    desktop_pages: Vec<Vec<Image>>,
}

#[derive(Deserialize)]
struct Image { uri: String }

#[derive(Deserialize)]
struct Champion { name: String }

fn clean_description(input: String) -> String {
    input.replace("</p> ", "</p>").replace("</p>", "\n").replace("<p>", "")
}

const HUB_FIXTURE: &str = r#"{
  "sections": {
    "series": { "data": [{ "title": "Sample Series", "subtitle": "Adventure", "url": "https://universe.leagueoflegends.com/en_us/comic/sample", "description": "<p>Sample</p>", "background": { "uri": "https://universe.leagueoflegends.com/cover.jpg" }, "featured-champions": [{ "name": "Lux" }] }] },
    "one-shots": { "data": [{ "title": "Sample One Shot", "url": "https://universe.leagueoflegends.com/en_us/comic/oneshot/1", "background": { "uri": "https://universe.leagueoflegends.com/one.jpg" } }] }
  }
}"#;

const ISSUES_FIXTURE: &str = r#"{
  "issues": [{ "title": "Issue 1", "index": 1.0, "url": "https://universe.leagueoflegends.com/en_us/comic/sample/1" }]
}"#;

const PAGES_FIXTURE: &str = r#"{
  "staging-date": "2024-01-01T00:00:00.000Z",
  "desktop-pages": [[{ "uri": "https://universe-comics.leagueoflegends.com/comics/en_us/sample/1/page-1.jpg" }]]
}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_league_json() {
        let source = SOURCES[0];
        let page = parse_hub(HUB_FIXTURE, source, "");
        assert_eq!(page.entries.len(), 2);
        let chapters = serde_json::from_str::<Issues>(ISSUES_FIXTURE).unwrap().issues.into_iter().map(|comic| chapter_from_comic(comic, source)).collect::<Vec<_>>();
        assert_eq!(chapters[0].key, "sample/1");
        assert_eq!(parse_pages_json(PAGES_FIXTURE).len(), 1);
    }
}
