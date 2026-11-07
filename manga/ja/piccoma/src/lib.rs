// SPDX-License-Identifier: Apache-2.0
// Piccoma behavior adapted from keiyoushi/extensions-source (Apache-2.0),
// src/ja/piccoma. See NOTICE for attribution.
mod scramble;

use std::collections::BTreeSet;

use manatan_sdk::{
    client::Client, context, AuthenticationAction, AuthenticationRequest, AuthenticationState,
    AuthenticationWebViewCompletion, CatalogItem, Error, FilterDefinition, ImageRequest,
    MangaChapter, MangaPage, MangaSource, OptionItem, PageContent, Paged, PreferenceDefinition,
    ProcessedImage, Result, UrlResolveResult,
};
use regex::Regex;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

const BASE: &str = "https://piccoma.com";
const CDN: &str = "piccoma.kakaocdn.net";
const SIGN_IN: &str = "https://piccoma.com/web/acc/signin";
const RANKINGS: &[(&str, &str)] = &[
    ("マンガ・総合", "K/P/0"),
    ("マンガ・ファンタジー", "K/P/2"),
    ("マンガ・恋愛", "K/P/1"),
    ("マンガ・アクション", "K/P/5"),
    ("マンガ・ドラマ", "K/P/3"),
    ("マンガ・ホラー・ミステリー", "K/P/7"),
    ("マンガ・スポーツ", "K/P/6"),
    ("マンガ・グルメ", "K/P/10"),
    ("マンガ・日常", "K/P/4"),
    ("マンガ・雑誌", "K/P/16"),
    ("マンガ・TL", "K/P/13"),
    ("マンガ・BL", "K/P/14"),
    ("SMARTOON・総合", "S/P/0"),
    ("SMARTOON・ファンタジー", "S/P/2"),
    ("SMARTOON・恋愛", "S/P/1"),
    ("SMARTOON・アクション", "S/P/5"),
    ("SMARTOON・ドラマ", "S/P/3"),
    ("SMARTOON・ホラー・ミステリー", "S/P/7"),
    ("SMARTOON・スポーツ", "S/P/6"),
    ("SMARTOON・グルメ", "S/P/10"),
    ("SMARTOON・日常", "S/P/4"),
    ("SMARTOON・TL", "S/P/13"),
    ("SMARTOON・BL", "S/P/14"),
];

pub struct PiccomaSource {
    client: Client,
}
impl Default for PiccomaSource {
    fn default() -> Self {
        Self {
            client: Client::browser().header("Referer", format!("{BASE}/web/")),
        }
    }
}
impl PiccomaSource {
    fn text(&self, url: &str, ajax: bool) -> Result<String> {
        let mut request = self
            .client
            .get(url)
            .max_body_bytes(4_000_000)
            .rate_limit("piccoma", 250);
        if ajax {
            request = request.header("X-Requested-With", "XMLHttpRequest");
        }
        let response = request.send()?.error_for_status()?;
        let body = response.text()?.to_owned();
        check_page(&body)?;
        Ok(body)
    }
    fn ranking(&self, path: &str, page: u32) -> Result<Paged<CatalogItem>> {
        if page > 1 {
            return Ok(Paged::default());
        }
        if !RANKINGS.iter().any(|(_, allowed)| *allowed == path) {
            return Err(Error::new("Unknown Piccoma ranking"));
        }
        parse_listing(
            &self.text(&format!("{BASE}/web/ranking/{path}"), false)?,
            true,
        )
    }
}
impl MangaSource for PiccomaSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        self.ranking("K/P/0", page)
    }
    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = format!("{BASE}/web/weekday/product/list?page={}", page.max(1));
        parse_listing(&self.text(&url, false)?, false)
    }
    fn listing(&mut self, listing: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        match listing {
            "popular" => self.ranking(ranking_filter(filters)?, page),
            "latest" => self.latest(page),
            _ => Err(Error::new("Unknown Piccoma listing")),
        }
    }
    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        if query.trim().starts_with("https://") {
            return Ok(Paged::new(
                self.handle_url(query.trim())?
                    .and_then(|r| r.item)
                    .into_iter()
                    .collect(),
                false,
            ));
        }
        if query.trim().is_empty() {
            return self.ranking(ranking_filter(filters)?, page);
        }
        let mut url = Url::parse(&format!("{BASE}/web/search/result_ajax/list")).map_err(err)?;
        url.query_pairs_mut()
            .append_pair("word", query.trim())
            .append_pair("page", &page.max(1).to_string())
            .append_pair("tab_type", "T");
        parse_search(&self.text(url.as_str(), true)?, page.max(1))
    }
    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let id = product_id(&item)?;
        parse_details(&self.text(&product_url(&id), false)?, &id)
    }
    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let id = product_id(&item)?;
        let hide_locked = context::preference::<bool>("hide_locked")?.unwrap_or(false);
        let mut chapters = Vec::new();
        for kind in ["V", "E"] {
            let html = self.text(
                &format!("{}/episodes?etype={kind}", product_url(&id)),
                false,
            )?;
            chapters.extend(parse_chapters(&html, &id, kind, hide_locked)?);
        }
        if chapters.is_empty() && !hide_locked {
            return Err(Error::new(
                "Piccoma returned no chapters. The title may be app-only or unavailable.",
            ));
        }
        chapters.reverse();
        Ok(chapters)
    }
    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        let id = product_id(&item)?;
        let episode = chapter_id(&chapter)?;
        let viewer = format!("{BASE}/web/viewer/{id}/{episode}");
        parse_pages(&self.text(&viewer, false)?, &viewer)
    }
    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(vec![FilterDefinition::Select {
            id: "ranking".into(),
            name: "ランキング".into(),
            default_index: 0,
            options: RANKINGS
                .iter()
                .map(|(label, value)| OptionItem {
                    label: (*label).into(),
                    value: (*value).into(),
                })
                .collect(),
        }])
    }
    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(vec![PreferenceDefinition::Switch {
            key: "hide_locked".into(), title: "Hide locked chapters".into(), default: false,
            summary: Some("Hide chapters requiring purchase or a wait-free claim on Piccoma. Free volume previews remain available.".into()),
        }])
    }
    fn authentication_status(&mut self) -> Result<AuthenticationState> {
        parse_authentication_status(&self.text(&format!("{BASE}/web/"), false)?)
    }
    fn authenticate(&mut self, request: AuthenticationRequest) -> Result<AuthenticationState> {
        if !request.interactive {
            return self.authentication_status();
        }
        Ok(AuthenticationState {
            message: Some("Sign in on Piccoma. Purchases and wait-free claims must be completed on the website. The service requires access from Japan.".into()),
            action: Some(AuthenticationAction::WebView {
                url: SIGN_IN.into(), profile: "login".into(), cookie_url: Some(format!("{BASE}/")),
                completion: AuthenticationWebViewCompletion::default(),
            }), ..Default::default()
        })
    }
    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(product_url(&product_id(item)?)))
    }
    fn chapter_url(
        &mut self,
        item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        Ok(Some(format!(
            "{BASE}/web/viewer/{}/{}",
            product_id(item)?,
            chapter_id(chapter)?
        )))
    }
    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        resolve_url(candidate)
    }
    fn process_page_image(
        &mut self,
        _item: &CatalogItem,
        _chapter: &MangaChapter,
        page: &MangaPage,
        bytes: &[u8],
        _mime: Option<&str>,
    ) -> Result<Option<ProcessedImage>> {
        let Some(seed) = page.extra.get("piccomaSeed").and_then(Value::as_str) else {
            return Ok(None);
        };
        Ok(Some(ProcessedImage {
            bytes: scramble::decode(bytes, seed)?,
            mime_type: Some("image/png".into()),
        }))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("piccoma", PiccomaSource::default())
);

fn err(error: impl std::fmt::Display) -> Error {
    Error::new(error.to_string())
}
fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("constant CSS selector")
}
fn text(node: ElementRef<'_>) -> String {
    node.text()
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn first_text(root: &Html, css: &str) -> Option<String> {
    root.select(&selector(css))
        .next()
        .map(text)
        .filter(|s| !s.is_empty())
}
fn check_page(html: &str) -> Result<()> {
    if html.contains("PCM-error_overseasAccess")
        || html.contains("this service can only be used from Japan")
    {
        return Err(Error::new(
            "Piccoma is available only in Japan; the server rejected access from this network.",
        ));
    }
    Ok(())
}
fn numeric(value: &str) -> Result<String> {
    if !value.is_empty() && value.len() <= 20 && value.bytes().all(|b| b.is_ascii_digit()) {
        Ok(value.into())
    } else {
        Err(Error::new("Invalid Piccoma product or chapter identifier"))
    }
}
fn product_url(id: &str) -> String {
    format!("{BASE}/web/product/{id}")
}
fn product_id(item: &CatalogItem) -> Result<String> {
    numeric(&item.key).or_else(|_| {
        item.url
            .as_deref()
            .and_then(|s| resolve_url(s).ok().flatten())
            .and_then(|r| r.item)
            .map(|i| i.key)
            .ok_or_else(|| Error::new("Missing Piccoma product identifier"))
    })
}
fn chapter_id(chapter: &MangaChapter) -> Result<String> {
    numeric(
        chapter
            .extra
            .get("episodeId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| chapter.key.rsplit('/').next().unwrap_or("")),
    )
}
fn resolve_url(candidate: &str) -> Result<Option<UrlResolveResult>> {
    let url = Url::parse(candidate).map_err(err)?;
    if url.scheme() != "https"
        || url.host_str() != Some("piccoma.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Ok(None);
    }
    let segments = url.path().trim_matches('/').split('/').collect::<Vec<_>>();
    let (id, episode) = match segments.as_slice() {
        ["web", "product", id] | ["web", "product", id, "episodes"] => (numeric(id)?, None),
        ["web", "viewer", id, episode] => (numeric(id)?, Some(numeric(episode)?)),
        _ => return Ok(None),
    };
    let mut item = CatalogItem::new(&id, format!("Piccoma {id}"));
    item.url = Some(product_url(&id));
    item.language = Some("ja".into());
    item.content_rating = Some("adult".into());
    let chapter = episode.map(|episode| MangaChapter {
        key: episode.clone(),
        url: Some(format!("{BASE}/web/viewer/{id}/{episode}")),
        extra: [("episodeId".into(), json!(episode))].into(),
        ..Default::default()
    });
    Ok(Some(UrlResolveResult {
        item: Some(item),
        chapter_key: chapter.as_ref().map(|c| c.key.clone()),
        manga_chapter: chapter,
        ..Default::default()
    }))
}
fn image_url(raw: &str) -> Result<String> {
    let value = if raw.starts_with("//") {
        format!("https:{raw}")
    } else {
        raw.to_owned()
    };
    let url = Url::parse(BASE).expect("base").join(&value).map_err(err)?;
    if url.scheme() != "https"
        || !matches!(url.host_str(), Some("piccoma.com") | Some(CDN))
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::new(
            "Piccoma returned an image from an undeclared origin",
        ));
    }
    Ok(url.into())
}
fn cover_url(raw: &str) -> Result<String> {
    let mut url = Url::parse(&image_url(raw)?).map_err(err)?;
    let last = url
        .path_segments()
        .and_then(|mut p| p.next_back())
        .unwrap_or("")
        .to_owned();
    if last.starts_with("thumbnail_x") || last.starts_with("cover_x") {
        url.path_segments_mut()
            .map_err(|_| Error::new("Invalid cover URL"))?
            .pop()
            .push("cover_x3");
    }
    Ok(url.into())
}
fn image_request(url: String) -> ImageRequest {
    ImageRequest {
        url,
        headers: [("Referer".into(), format!("{BASE}/web/"))].into(),
        ..Default::default()
    }
}
fn make_item(id: &str, title: String, cover: Option<&str>) -> Result<CatalogItem> {
    if title.trim().is_empty() {
        return Err(Error::new("Piccoma catalog title is empty"));
    }
    let mut item = CatalogItem::new(numeric(id)?, title);
    item.url = Some(product_url(id));
    item.language = Some("ja".into());
    item.content_rating = Some("adult".into());
    item.cover = cover
        .filter(|s| !s.is_empty())
        .map(cover_url)
        .transpose()?
        .map(image_request);
    Ok(item)
}
fn parse_listing(html: &str, ranking: bool) -> Result<Paged<CatalogItem>> {
    check_page(html)?;
    let doc = Html::parse_document(html);
    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    let css = if ranking {
        "section.PCM-productRanking li > a[href]"
    } else {
        "li a[href]:has(.PCOM-prdList_info)"
    };
    for node in doc.select(&selector(css)) {
        let Some(resolved) = node
            .value()
            .attr("href")
            .and_then(|u| Url::parse(BASE).ok()?.join(u).ok())
            .and_then(|u| resolve_url(u.as_str()).ok().flatten())
        else {
            continue;
        };
        let Some(item) = resolved.item else {
            continue;
        };
        if !seen.insert(item.key.clone()) {
            continue;
        }
        let title = node
            .select(&selector(
                ".PCM-rankingProduct_title p, .PCOM-prdList_title span",
            ))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::new("Piccoma catalog title is missing"))?;
        let cover = node.select(&selector("img")).next().and_then(|i| {
            i.value()
                .attr("data-original")
                .or_else(|| i.value().attr("src"))
        });
        entries.push(make_item(&item.key, title, cover)?);
    }
    if entries.is_empty()
        && doc
            .select(&selector(
                "section.PCM-productRanking, #js_productList, .PCOM-productList",
            ))
            .next()
            .is_none()
    {
        return Err(Error::new(
            "Piccoma catalog markup is unavailable or changed",
        ));
    }
    Ok(Paged::new(
        entries,
        !ranking && doc.select(&selector("#js_nextPage")).next().is_some(),
    ))
}
#[derive(Deserialize)]
struct SearchResponse {
    status: i32,
    data: SearchData,
}
#[derive(Deserialize)]
struct SearchData {
    products: Vec<SearchProduct>,
    total_page: u32,
}
#[derive(Deserialize)]
struct SearchProduct {
    id: u64,
    title: String,
    img: Option<String>,
    #[serde(default)]
    is_audio: Option<u8>,
    #[serde(default)]
    is_anime: Option<u8>,
    #[serde(default)]
    is_novel: Option<u8>,
}
fn parse_search(body: &str, page: u32) -> Result<Paged<CatalogItem>> {
    let response: SearchResponse = serde_json::from_str(body)
        .map_err(|_| Error::new("Piccoma search returned an invalid response"))?;
    if response.status != 0 {
        return Err(Error::new("Piccoma search was rejected by the server"));
    }
    let mut seen = BTreeSet::new();
    let entries = response
        .data
        .products
        .into_iter()
        .filter(|p| {
            p.is_audio.unwrap_or(0) == 0
                && p.is_anime.unwrap_or(0) == 0
                && p.is_novel.unwrap_or(0) == 0
                && seen.insert(p.id)
        })
        .map(|p| make_item(&p.id.to_string(), p.title, p.img.as_deref()))
        .collect::<Result<_>>()?;
    Ok(Paged::new(entries, page < response.data.total_page))
}
fn ranking_filter(filters: &Value) -> Result<&str> {
    let selected = filters
        .get("ranking")
        .and_then(Value::as_str)
        .unwrap_or("K/P/0");
    RANKINGS
        .iter()
        .find(|(_, path)| *path == selected)
        .map(|(_, path)| *path)
        .ok_or_else(|| Error::new("Unknown Piccoma ranking"))
}
fn parse_details(html: &str, id: &str) -> Result<CatalogItem> {
    check_page(html)?;
    let doc = Html::parse_document(html);
    let title = first_text(&doc, "h1.PCM-productTitle").ok_or_else(|| {
        Error::new("Piccoma product title is missing; the title may be unavailable")
    })?;
    let cover = doc
        .select(&selector("img.PCM-productThum_img"))
        .next()
        .and_then(|i| {
            i.value()
                .attr("data-original")
                .or_else(|| i.value().attr("src"))
        });
    let mut item = make_item(id, title, cover)?;
    item.authors = doc
        .select(&selector("ul.PCM-productAuthor li a"))
        .map(text)
        .filter(|s| !s.is_empty())
        .collect();
    item.tags = doc
        .select(&selector(
            "ul.PCM-productGenre li a, .PCM-productDesc_tagList li a",
        ))
        .map(text)
        .filter(|s| !s.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    item.description = first_text(&doc, "div.PCM-productDesc > p");
    item.status = first_text(&doc, "ul.PCM-productStatus").and_then(|s| {
        if s.contains("完結") {
            Some(json!("completed"))
        } else if s.contains("連載中") {
            Some(json!("ongoing"))
        } else {
            None
        }
    });
    item.initialized = true;
    Ok(item)
}
fn parse_chapters(
    html: &str,
    id: &str,
    kind: &str,
    hide_locked: bool,
) -> Result<Vec<MangaChapter>> {
    check_page(html)?;
    let doc = Html::parse_document(html);
    if doc
        .select(&selector(
            "#js_volumeList, #js_episodeList, .PCM-headTitle_name",
        ))
        .next()
        .is_none()
    {
        return Err(Error::new(
            "Piccoma chapter list is unavailable; open the title on the website to check access",
        ));
    }
    let manga_title = first_text(&doc, ".PCM-headTitle_name").unwrap_or_default();
    let css = if kind == "V" {
        "ul#js_volumeList > li"
    } else {
        "ul#js_episodeList > li"
    };
    let mut chapters = Vec::new();
    let mut seen = BTreeSet::new();
    let number_pattern =
        Regex::new(r"(?:第)?([0-9]+(?:\.[0-9]+)?)(?:話|巻)").expect("number regex");
    for node in doc.select(&selector(css)) {
        let free = node.select(&selector(".PCM-prdVol_freeBtn")).next();
        let trial = node.select(&selector(".PCM-prdVol_trialBtn")).next();
        let buy = node.select(&selector(".PCM-prdVol_buyBtn")).next();
        let button = free
            .or(trial)
            .or(buy)
            .and_then(|b| {
                if b.value().attr("data-episode_id").is_some() {
                    Some(b)
                } else {
                    b.select(&selector("[data-episode_id]")).next()
                }
            })
            .or_else(|| node.select(&selector("[data-episode_id]")).next());
        let Some(button) = button else {
            continue;
        };
        let episode = numeric(button.value().attr("data-episode_id").unwrap_or(""))?;
        if !seen.insert(episode.clone()) {
            continue;
        }
        if let Some(product) = button.value().attr("data-product_id") {
            if product != id {
                return Err(Error::new("Piccoma chapter belongs to a different product"));
            }
        }
        let waiting = node
            .select(&selector(
                ".PCM-epList_status_waitfree, .PCM-epList_status_zeroPlus",
            ))
            .next()
            .is_some();
        let locked = if kind == "V" {
            free.is_none() && trial.is_none()
        } else {
            waiting
                || node
                    .select(&selector(".PCM-epList_status_point"))
                    .next()
                    .is_some()
        };
        if hide_locked && locked {
            continue;
        }
        let title = node
            .select(&selector(
                "div.PCM-epList_title h2, div.PCM-prdVol_title h2",
            ))
            .next()
            .map(text)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::new("Piccoma chapter title is missing"))?;
        let title = title.strip_prefix(&manga_title).unwrap_or(&title).trim();
        let number = number_pattern
            .captures(title)
            .and_then(|c| c[1].parse::<f32>().ok());
        let preview = kind == "V" && free.is_none() && trial.is_some();
        chapters.push(MangaChapter {
            key: episode.clone(),
            title: Some(if preview {
                format!("(Preview) {title}")
            } else {
                title.into()
            }),
            chapter_number: if kind == "E" { number } else { None },
            volume_number: if kind == "V" { number } else { None },
            url: Some(format!("{BASE}/web/viewer/{id}/{episode}")),
            language: Some("ja".into()),
            is_locked: locked,
            extra: [
                ("episodeId".into(), json!(episode)),
                ("preview".into(), json!(preview)),
                ("waitFree".into(), json!(waiting)),
            ]
            .into(),
            ..Default::default()
        });
    }
    Ok(chapters)
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReaderData {
    img: Option<Vec<ReaderImage>>,
    contents: Option<Vec<ReaderImage>>,
    is_scrambled: bool,
}
#[derive(Deserialize)]
struct ReaderImage {
    path: String,
}

fn parse_authentication_status(html: &str) -> Result<AuthenticationState> {
    check_page(html)?;
    let document = Html::parse_document(html);
    let login = Regex::new(r#"\A\s*['\"]login['\"]\s*:\s*(true|false)\b"#).expect("login regex");
    for script in document.select(&selector("script")) {
        let script = script.inner_html();
        if let Some((_, tail)) = script.split_once("var _init_") {
            if let Some((_, fields)) = tail.split_once('{') {
                if let Some(value) = login.captures(fields) {
                    return Ok(AuthenticationState {
                        authenticated: &value[1] == "true",
                        message: Some("Piccoma controls chapter access. Purchases and wait-free claims must be completed on its website.".into()),
                        ..Default::default()
                    });
                }
            }
        }
    }
    Err(Error::new(
        "Piccoma did not return a recognizable login status",
    ))
}
fn reader_object(script: &str) -> Result<&str> {
    let tail = script
        .split_once("var _pdata_")
        .ok_or_else(|| {
            Error::new(
                "Piccoma requires website login, purchase, or a wait-free claim for this chapter",
            )
        })?
        .1;
    let tail = tail
        .split_once('=')
        .map(|(_, s)| s.trim_start())
        .filter(|s| s.starts_with('{'))
        .ok_or_else(|| Error::new("Invalid Piccoma reader metadata"))?;
    let mut quote = None;
    let mut escape = false;
    let mut depth = 0;
    for (index, ch) in tail.char_indices() {
        if let Some(q) = quote {
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == q {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(&tail[..index + 1]);
                }
            }
            _ => {}
        }
    }
    Err(Error::new("Truncated Piccoma reader metadata"))
}
fn parse_pages(html: &str, viewer: &str) -> Result<Vec<MangaPage>> {
    check_page(html)?;
    let doc = Html::parse_document(html);
    let script = doc
        .select(&selector("script"))
        .map(|n| n.inner_html())
        .find(|s| s.contains("var _pdata_"))
        .ok_or_else(|| {
            Error::new(
                "Piccoma requires website login, purchase, or a wait-free claim for this chapter",
            )
        })?;
    let data: ReaderData = json5::from_str(reader_object(&script)?)
        .map_err(|_| Error::new("Piccoma reader metadata format changed"))?;
    let images = data
        .img
        .filter(|v| !v.is_empty())
        .or(data.contents)
        .unwrap_or_default();
    let mut pages = Vec::new();
    for image in images.into_iter().filter(|i| !i.path.is_empty()) {
        let url = image_url(&image.path)?;
        let mut extra = manatan_sdk::Extra::default();
        if data.is_scrambled {
            extra.insert("piccomaSeed".into(), json!(scramble::seed(&url)?));
        }
        pages.push(MangaPage {
            content: PageContent::Url { url, context: None },
            headers: [("Referer".into(), viewer.into())].into(),
            extra,
            ..Default::default()
        });
    }
    if pages.is_empty() {
        return Err(Error::new(
            "Piccoma returned no readable pages for this chapter",
        ));
    }
    Ok(pages)
}

#[cfg(test)]
mod tests;
