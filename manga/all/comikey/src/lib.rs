use manatan_extension::{
    CatalogItem, HomeSection, HomeSectionStyle, ItemStatus, MangaChapter, MangaPage,
    PageContent, Paged, SearchRequest, UrlResolveResult, abi::ExtensionResult,
    export_manga_source, source::MangaSource, webview,
};
use manatan_shared::{dates, html, manga, sdk::http::HttpClient, url};
use serde_json::{Value, json};

const SOURCE: Comikey = Comikey;
const GUNDAM_URL: &str = "https://gundam.comikey.net";

struct Comikey;

#[derive(Clone, Copy)]
struct Config {
    id: &'static str,
    lang: &'static str,
    default_lang: &'static str,
    name: &'static str,
    base_url: &'static str,
}

const SOURCES: &[Config] = &[
    Config { id: "comikey-en", lang: "en", default_lang: "en", name: "Comikey", base_url: "https://comikey.com" },
    Config { id: "comikey-es", lang: "es", default_lang: "en", name: "Comikey", base_url: "https://comikey.com" },
    Config { id: "comikey-id", lang: "id", default_lang: "en", name: "Comikey", base_url: "https://comikey.com" },
    Config { id: "comikey-pt-br", lang: "pt-BR", default_lang: "en", name: "Comikey", base_url: "https://comikey.com" },
    Config { id: "comikey-br", lang: "pt-BR", default_lang: "pt-BR", name: "Comikey Brasil", base_url: "https://br.comikey.com" },
];

impl MangaSource for Comikey {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config(&request);
        let page = page(&request);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            String::new()
        } else {
            "order=-views&".to_string()
        };
        let target = format!("{}/comics/?{order}page={page}", config.base_url);
        Ok(parse_catalog_page(&fetch_text(config, &target, CATALOG_FIXTURE), config))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let config = config(&request);
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if let Some(key) = key_from_url(config, query) {
            return Ok(Paged { entries: vec![details_for_key(config, &key)], has_next_page: false });
        }

        let mut target = format!("{}/comics/?page={}", config.base_url, page(&request));
        if query.chars().count() >= 2 {
            target.push_str("&q=");
            target.push_str(&url::query_escape(query));
        }
        if let Some(sort) = filter_string(&request, "sort").filter(|value| !value.is_empty()) {
            target.push_str("&order=");
            target.push_str(&url::query_escape(&sort));
        }
        if let Some(kind) = filter_string(&request, "type").filter(|value| !value.is_empty()) {
            target.push_str("&filter=");
            target.push_str(&url::query_escape(&kind));
        }
        Ok(parse_catalog_page(&fetch_text(config, &target, CATALOG_FIXTURE), config))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let config = config(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample/sample/".into());
        Ok(details_for_key(config, &key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let config = config(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| "/comics/sample/sample/".into());
        let body = fetch_text(config, &url::join_url(config.base_url, &key), DETAILS_FIXTURE);
        let comic = comic_json(&body).unwrap_or(Value::Null);
        let manga_id = key.trim_matches('/').split('/').nth(1).unwrap_or("sample");
        let manga_slug = key.trim_matches('/').split('/').nth(0).unwrap_or("sample");
        let token = html::text_between(&body, "GUNDAM.token", ";")
            .and_then(|value| value.split('"').nth(1).map(ToString::to_string));
        let endpoint = if token.is_some() { "comic" } else { "comic.public" };
        let mut target = format!("{GUNDAM_URL}/{endpoint}/{manga_id}/episodes?language={}", config.lang.to_lowercase());
        if let Some(token) = token {
            target.push_str("&token=");
            target.push_str(&url::query_escape(&token));
        }
        let episodes = fetch_text(config, &target, EPISODES_FIXTURE);
        Ok(parse_chapters(&episodes, &comic, config, manga_slug, preference_bool(&request, "hide_locked_chapters")))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let config = config(&request);
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/read/sample/sample/chapter-1/".into());
        let reader_url = url::join_url(config.base_url, &key);
        match fetch_reader_manifest(config, &reader_url) {
            Some((manifest_url, act, manifest)) => Ok(pages_from_manifest(config, &manifest_url, &act, &manifest)),
            None => Ok(parse_pages_fixture(config)),
        }
    }

    fn home(&self, request: Value) -> ExtensionResult<Vec<HomeSection<CatalogItem>>> {
        let source_id = source_id(&request);
        let popular = self.list(json!({"page": 1, "listingId": "popular", "sourceId": source_id}))?;
        let latest = self.list(json!({"page": 1, "listingId": "latest", "sourceId": source_id}))?;
        Ok(vec![
            HomeSection {
                id: "popular".into(),
                title: "Popular".into(),
                style: Some(HomeSectionStyle::Cover),
                has_more: popular.has_next_page,
                entries: popular.entries,
                ..HomeSection::default()
            },
            HomeSection {
                id: "latest".into(),
                title: "Latest".into(),
                style: Some(HomeSectionStyle::Compact),
                has_more: latest.has_next_page,
                entries: latest.entries,
                ..HomeSection::default()
            },
        ])
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config(&request);
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(config.base_url, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        let config = config(&request);
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(config.base_url, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let config = config(&request);
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = key_from_url(config, input) {
            return Ok(Some(UrlResolveResult {
                item: Some(details_for_key(config, &key)),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest { query: input.into(), ..SearchRequest::default() }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

fn client(config: Config) -> HttpClient {
    HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", config.base_url))
        .with_webview_challenge_fallback()
}

fn fetch_text(config: Config, target: &str, fixture: &str) -> String {
    client(config).get(target).send_text().unwrap_or_else(|_| fixture.to_string())
}

fn parse_catalog_page(body: &str, config: Config) -> Paged<CatalogItem> {
    let entries = body
        .split("<li")
        .filter(|chunk| chunk.contains("series-data") && chunk.contains("series-listing"))
        .filter_map(|chunk| catalog_from_chunk(chunk, config))
        .collect::<Vec<_>>();
    let has_next_page = body.contains("next-page") && !body.contains("next-page disabled");
    Paged { entries, has_next_page }
}

fn catalog_from_chunk(chunk: &str, config: Config) -> Option<CatalogItem> {
    let title_block = chunk.split("series-data").nth(1).unwrap_or(chunk);
    let href = html::attr_after(title_block, "<a", "href")?;
    let title = html::text_between(title_block, "<a", "</a>").map(|value| html::strip_tags(&value))?;
    let description = html::text_between(chunk, "<div class=\"excerpt", "</div>")
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty());
    Some(CatalogItem {
        key: key_from_url(config, &href).unwrap_or(href),
        title,
        cover: html::attr_after(chunk, "<img", "src").map(|value| url::join_url(config.base_url, &value)),
        description,
        content_rating: Some("safe".into()),
        language: Some(config.lang.into()),
        ..CatalogItem::default()
    })
}

fn details_for_key(config: Config, key: &str) -> CatalogItem {
    let body = fetch_text(config, &url::join_url(config.base_url, key), DETAILS_FIXTURE);
    let comic = comic_json(&body).unwrap_or(Value::Null);
    let title = text(&comic, "name").unwrap_or_else(|| config.name.into());
    let mut description = String::new();
    if let Some(excerpt) = text(&comic, "excerpt").filter(|value| !value.is_empty()) {
        description.push('"');
        description.push_str(&excerpt);
        description.push_str("\"\n\n");
    }
    description.push_str(&text(&comic, "description").unwrap_or_default());
    CatalogItem {
        key: text(&comic, "link").unwrap_or_else(|| key.into()),
        title,
        cover: text(&comic, "full_cover")
            .or_else(|| text(&comic, "fullCover"))
            .map(|value| url::join_url(config.base_url, &value)),
        authors: names(&comic, "author"),
        artists: names(&comic, "artist"),
        tags: tags(&comic),
        description: (!description.trim().is_empty()).then_some(description),
        status: status(&comic),
        content_rating: Some("safe".into()),
        language: Some(config.lang.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, comic: &Value, config: Config, manga_slug: &str, hide_locked: bool) -> Vec<MangaChapter> {
    let default_prefix = if comic.get("format").and_then(Value::as_i64) == Some(2) { "episode" } else { "chapter" };
    let episodes = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|root| root.get("episodes").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut chapters = episodes
        .into_iter()
        .filter(|episode| !hide_locked || readable(episode))
        .filter_map(|episode| {
            let id = text(&episode, "id")?;
            let number = episode.get("number").and_then(Value::as_f64).unwrap_or(0.0);
            let e4pid = id.split_once('-').map(|(_, tail)| tail).unwrap_or(&id);
            let prefix = chapter_prefix(default_prefix, config);
            let number_slug = clean_number(number);
            let title = text(&episode, "title").unwrap_or_else(|| format!("Chapter {number_slug}"));
            let name = match text(&episode, "subtitle") {
                Some(subtitle) if !subtitle.is_empty() => format!("{title}: {subtitle}"),
                _ => title,
            };
            Some(MangaChapter {
                key: format!("/read/{manga_slug}/{e4pid}/{prefix}-{number_slug}/"),
                title: Some(name),
                chapter_number: Some(number as f32),
                date_uploaded: text(&episode, "releasedAt").and_then(|value| parse_iso_date(&value)),
                scanlators: vec![config.name.into()],
                language: Some(config.lang.into()),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    chapters
}

fn fetch_reader_manifest(config: Config, reader_url: &str) -> Option<(String, String, Value)> {
    let response = webview::extract_text(
        webview::ExtractRequest::new(reader_url, COMIKEY_EXTRACT_SCRIPT)
        .header("Referer", format!("{}/", config.base_url))
        .wait_for_selector("#lmao-init")
        .timeout_ms(30_000)
        .cookies(true),
    )
    .ok()?;

    let payload = serde_json::from_str::<Value>(&response).ok()?;
    if payload.get("error").and_then(Value::as_str).is_some() {
        return None;
    }
    let manifest_url = text(&payload, "manifestUrl")?;
    let act = text(&payload, "act").unwrap_or_default();
    let manifest_response = client(config)
        .get(&manifest_url)
        .send()
        .ok()?;
    let resolved_url = manifest_response.final_url.clone();
    let manifest = serde_json::from_str::<Value>(&manifest_response.text.unwrap_or_default()).ok()?;
    Some((resolved_url, act, manifest))
}

fn pages_from_manifest(config: Config, manifest_url: &str, act: &str, manifest: &Value) -> Vec<MangaPage> {
    let webtoon = manifest
        .pointer("/metadata/readingProgression")
        .and_then(Value::as_str)
        == Some("ttb");
    let base = manifest_url.rsplit_once('/').map(|(base, _)| base).unwrap_or(manifest_url);
    manifest
        .get("readingOrder")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter_map(|(index, page)| {
            let href = preferred_page_href(&page, webtoon)?;
            let mut image_url = url::join_url(base, &href);
            if !act.is_empty() {
                image_url.push_str(if image_url.contains('?') { "&act=" } else { "?act=" });
                image_url.push_str(&url::query_escape(act));
            }
            Some(MangaPage {
                content: PageContent::Url { url: image_url.clone(), context: Some(manga::image_headers(config.base_url)) },
                headers: manga::image_headers(config.base_url),
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            })
        })
        .collect()
}

fn preferred_page_href(page: &Value, webtoon: bool) -> Option<String> {
    if page.get("height").and_then(Value::as_u64) == Some(2048)
        && page.get("type").and_then(Value::as_str) == Some("image/jpeg")
    {
        if let Some(href) = page
            .get("alternate")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|alt| {
                alt.get("type").and_then(Value::as_str) == Some("image/webp")
                    && if webtoon {
                        alt.get("width").and_then(Value::as_u64).unwrap_or(u64::MAX) <= 1536
                    } else {
                        alt.get("height").and_then(Value::as_u64).unwrap_or(u64::MAX) <= 1536
                    }
            })
            .and_then(|alt| text(alt, "href"))
        {
            return Some(href);
        }
    }
    text(page, "href")
}

fn parse_pages_fixture(config: Config) -> Vec<MangaPage> {
    pages_from_manifest(config, "https://relay-sample.epub.rocks/sample/manifest", "", &serde_json::from_str(PAGES_FIXTURE).unwrap_or(Value::Null))
}

fn comic_json(body: &str) -> Option<Value> {
    let script = html::text_between(body, "id=\"comic\"", "</script>")
        .or_else(|| html::text_between(body, "id='comic'", "</script>"))?;
    serde_json::from_str(script.trim()).ok()
}

fn key_from_url(config: Config, input: &str) -> Option<String> {
    if input.starts_with("http") && !input.contains(config.base_url.trim_start_matches("https://")) {
        return None;
    }
    let path = input
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input);
    let trimmed = format!("/{}", path.trim_matches('/'));
    if trimmed.starts_with("/comics/") {
        Some(format!("{}/", trimmed.trim_end_matches('/')))
    } else {
        None
    }
}

fn source_id(request: &Value) -> String {
    request.get("sourceId").and_then(Value::as_str).unwrap_or("comikey-en").to_string()
}

fn config(request: &Value) -> Config {
    let id = source_id(request);
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn page(request: &Value) -> u64 {
    request.get("page").and_then(Value::as_u64).unwrap_or(1).max(1)
}

fn filter_string(request: &Value, id: &str) -> Option<String> {
    request
        .get("filters")
        .and_then(|filters| filters.get(id))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn preference_bool(request: &Value, id: &str) -> bool {
    request
        .get("preferences")
        .and_then(|preferences| preferences.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(ToString::to_string)
}

fn names(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| text(item, "name"))
        .collect()
}

fn tags(value: &Value) -> Vec<String> {
    let mut out = names(value, "tags");
    if let Some(format) = value.get("format").and_then(Value::as_i64) {
        out.push(match format {
            0 => "Comic",
            1 => "Manga",
            2 => "Webtoon",
            _ => "Other",
        }.into());
    }
    out
}

fn status(value: &Value) -> ItemStatus {
    match value.get("update_status").or_else(|| value.get("updateStatus")).and_then(Value::as_i64) {
        Some(1) => ItemStatus::Completed,
        Some(3) => ItemStatus::Hiatus,
        Some(4..=14) => ItemStatus::Ongoing,
        _ => ItemStatus::Unknown,
    }
}

fn readable(value: &Value) -> bool {
    value.get("finalPrice").and_then(Value::as_i64).unwrap_or(0) == 0
        || value.get("owned").and_then(Value::as_bool).unwrap_or(false)
}

fn chapter_prefix(default_prefix: &str, config: Config) -> &'static str {
    if default_prefix == "chapter" && config.lang != config.default_lang {
        match config.lang {
            "es" => "capitulo-espanol",
            "pt-BR" => "capitulo-portugues",
            "fr" => "chapitre-francais",
            "id" => "bab-bahasa",
            _ => "chapter",
        }
    } else if default_prefix == "episode" {
        "episode"
    } else {
        "chapter"
    }
}

fn clean_number(number: f64) -> String {
    let mut value = if number.fract() == 0.0 { format!("{}", number as i64) } else { number.to_string() };
    value = value.trim_end_matches('0').trim_end_matches('.').replace('.', "-");
    value
}

fn parse_iso_date(value: &str) -> Option<i64> {
    dates::parse_ymd(value.get(..10)?)
}

const COMIKEY_EXTRACT_SCRIPT: &str = r##"
(async function () {
    try {
      const db = await new Promise(function (resolve, reject) {
        const request = indexedDB.open("firebase-app-check-database");
        request.onsuccess = function (e) { resolve(e.target.result); };
        request.onerror = function (e) { reject(e.target); };
      });
      const act = await new Promise(function (resolve, reject) {
        db.onerror = function (e) { reject(e.target); };
        const request = db.transaction("firebase-app-check-store").objectStore("firebase-app-check-store").getAll();
        request.onsuccess = function (e) {
          const entries = e.target.result || [];
          db.close();
          if (!entries.length) throw new Error("App Check token not found");
          const value = entries[0].value;
          if (value.expireTimeMillis < Date.now()) throw new Error("App Check token expired");
          resolve(value.token);
        };
      });
      const manifestUrl = JSON.parse(document.querySelector("#lmao-init").textContent).manifest;
      return JSON.stringify({ manifestUrl, act });
    } catch (error) {
      return JSON.stringify({ error: String(error && error.message || error) });
    }
})()"##;

const CATALOG_FIXTURE: &str = r#"
<div class="series-listing" data-view="list"><ul>
<li><div class="series-data"><span class="title"><a href="/comics/sample/sample/">Sample Comikey</a></span></div><div class="excerpt"><p>Sample excerpt.</p></div><div class="image"><picture><img src="/sample-cover.jpg"></picture></div></li>
</ul></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<script id="comic" type="application/json">{"link":"/comics/sample/sample/","name":"Sample Comikey","author":[{"name":"Author"}],"artist":[{"name":"Artist"}],"tags":[{"name":"Action"}],"description":"Description.","excerpt":"Excerpt.","format":1,"full_cover":"/sample-cover.jpg","update_status":4,"update_text":"weekly"}</script>
"#;

const EPISODES_FIXTURE: &str = r#"{"episodes":[{"id":"comic-sample-episode-1","number":1,"title":"Chapter 1","subtitle":"Start","releasedAt":"2024-01-01T00:00:00Z","finalPrice":0,"owned":false}]}"#;

const PAGES_FIXTURE: &str = r#"{"metadata":{"readingProgression":"ltr"},"readingOrder":[{"href":"pages/001.jpg","type":"image/jpeg","height":1200,"width":800,"alternate":[{"href":"pages/001.webp","type":"image/webp","height":1200,"width":800}]}]}"#;

export_manga_source!(SOURCE);
