use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::{ExtensionError, ExtensionResult, cookies_get},
    export_manga_source,
    source::MangaSource,
};
use manatan_shared::{
    manga,
    sdk::{SearchRequest, http::HttpClient},
    url,
};
use serde::Deserialize;
use serde_json::{Value, json};

const SOURCE: Ono = Ono;
const BASE_URL: &str = "https://www.ono.live";
const API_URL: &str = "https://ws.ono.live/graphql";
const COGNITO_CLIENT_ID: &str = "12kanvg0bocd5hjtuul46phv7s";

struct Ono;

impl MangaSource for Ono {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_ranking(RANKING_FIXTURE));
        }
        Ok(parse_ranking(&graphql_or_fixture(
            "getCatalogRanking",
            RANKING_QUERY,
            json!({ "genreSlug": Value::Null }),
            RANKING_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(BASE_URL) {
            let key = normalize_key(query);
            return Ok(Paged {
                entries: vec![details_from_key(&key)],
                has_next_page: false,
            });
        }
        Ok(parse_search(&graphql_or_fixture(
            "searchCatalogByTerm",
            SEARCH_QUERY,
            json!({ "term": query }),
            SEARCH_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        Ok(details_from_key(&key))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(sample_key);
        let body = fetch_rsc_or_fixture(&absolute_url(&key), DETAILS_FIXTURE);
        let series = parse_series_detail(&body).unwrap_or_else(sample_series);
        let show_premium = pref_bool(&request, "showPremium", false);
        let show_wait_until_free = pref_bool(&request, "showWaitUntilFree", true);
        Ok(chapters_from_series(
            &series,
            show_premium,
            show_wait_until_free,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/manga/sample/1".to_string());
        let (slug, num) = slug_num_from_chapter_key(&key);
        let payload = reading_session(&slug, &num)?;
        Ok(pages_from_session(payload, &slug, &num)?)
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| absolute_url(&key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| absolute_url(&key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if input.starts_with(BASE_URL) && (input.contains("/manga/") || input.contains("/webtoon/"))
        {
            let key = normalize_key(input);
            return Ok(Some(UrlResolveResult {
                item: Some(details_from_key(&key)),
                url: Some(input.to_string()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: url::slug_from_url(input).unwrap_or_else(|| input.to_string()),
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
        .with_origin(BASE_URL)
        .with_referer(format!("{BASE_URL}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn graphql_or_fixture(operation: &str, query: &str, variables: Value, fixture: &str) -> String {
    let body = json!({
        "query": query,
        "operationName": operation,
        "variables": variables
    })
    .to_string();
    let client = client();
    let mut request = client
        .post(API_URL)
        .header("ono-platform", "website")
        .header("ono-product", "FR")
        .json(body)
        .xhr();
    if let Some(token) = bearer_token() {
        request = request.header("Authorization", format!("bearer {token}"));
    }
    request.send_text().unwrap_or_else(|_| fixture.to_string())
}

fn fetch_rsc_or_fixture(target: &str, fixture: &str) -> String {
    let client = client();
    client
        .get(target)
        .header("RSC", "1")
        .header("rsc", "1")
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn bearer_token() -> Option<String> {
    let response = cookies_get(BASE_URL).ok()?;
    let prefix = format!("CognitoIdentityServiceProvider.{COGNITO_CLIENT_ID}");
    let sub = response
        .cookies
        .iter()
        .find(|cookie| cookie.name == format!("{prefix}.LastAuthUser"))
        .map(|cookie| cookie.value.clone())?;
    response
        .cookies
        .into_iter()
        .find(|cookie| cookie.name == format!("{prefix}.{sub}.idToken") && !cookie.value.is_empty())
        .map(|cookie| cookie.value)
}

fn parse_ranking(body: &str) -> Paged<CatalogItem> {
    let payload = serde_json::from_str::<GraphQlResponse<RankingData>>(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .and_then(|data| data.get_catalog_ranking)
            .map(|ranking| {
                ranking
                    .series
                    .into_iter()
                    .map(RankingSeries::into_catalog)
                    .collect()
            })
            .unwrap_or_default(),
        has_next_page: false,
    }
}

fn parse_search(body: &str) -> Paged<CatalogItem> {
    let payload =
        serde_json::from_str::<GraphQlResponse<SearchCatalogData>>(body).unwrap_or_default();
    Paged {
        entries: payload
            .data
            .and_then(|data| data.search_catalog_by_term)
            .map(|result| {
                result
                    .series
                    .into_iter()
                    .map(SearchSeries::into_catalog)
                    .collect()
            })
            .unwrap_or_default(),
        has_next_page: false,
    }
}

fn details_from_key(key: &str) -> CatalogItem {
    let body = fetch_rsc_or_fixture(&absolute_url(key), DETAILS_FIXTURE);
    parse_series_detail(&body)
        .map(SeriesDetail::into_catalog)
        .unwrap_or_else(|| fallback_item(key))
}

fn parse_series_detail(body: &str) -> Option<SeriesDetail> {
    extract_value_with_keys(body, &["seriesElements", "contentType", "slug"])
        .and_then(|value| serde_json::from_value(value).ok())
}

fn chapters_from_series(
    series: &SeriesDetail,
    show_premium: bool,
    show_wait_until_free: bool,
) -> Vec<MangaChapter> {
    let base_key = series.key();
    let mut chapters = series
        .series_elements
        .iter()
        .filter_map(|element| {
            let locked = element.price.as_deref().is_some_and(|price| price != "0")
                && element.is_bought != Some(true);
            let wait_until_free = locked && element.wait_and_read.is_some();
            let premium = locked && !wait_until_free;
            if wait_until_free && !show_wait_until_free {
                return None;
            }
            if premium && !show_premium {
                return None;
            }
            let label = element
                .title
                .as_deref()
                .filter(|title| !title.trim().is_empty())
                .map(str::trim)
                .unwrap_or_else(|| element.num.as_str());
            let prefix = if wait_until_free {
                "Wait until free - "
            } else if premium {
                "Premium - "
            } else {
                ""
            };
            let key = format!("{}/{}", base_key.trim_end_matches('/'), element.num);
            Some(MangaChapter {
                key: key.clone(),
                title: Some(format!("{prefix}{label}")),
                chapter_number: element.num.parse::<f32>().ok(),
                url: Some(absolute_url(&key)),
                is_locked: premium,
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.sort_by(|a, b| {
        b.chapter_number
            .partial_cmp(&a.chapter_number)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    chapters
}

fn reading_session(slug: &str, num: &str) -> ExtensionResult<ReadingSessionPayload> {
    let mut payload = start_reading(slug, num)?;
    if payload.typename == "UserHasNotAccess" {
        if let Some(publication_id) = payload
            .publication_access_methods
            .iter()
            .find(|method| {
                method.typename == "WaitNReadAvailable" && method.publication_id.is_some()
            })
            .and_then(|method| method.publication_id.clone())
        {
            unlock_wait_until_free(&publication_id)?;
            payload = start_reading(slug, num)?;
        }
    }
    Ok(payload)
}

fn start_reading(slug: &str, num: &str) -> ExtensionResult<ReadingSessionPayload> {
    let body = graphql_or_fixture(
        "StartReadingSession",
        START_READING_QUERY,
        json!({ "slug": slug, "num": num }),
        READING_FIXTURE,
    );
    serde_json::from_str::<GraphQlResponse<StartReadingSessionData>>(&body)
        .ok()
        .and_then(|payload| payload.data)
        .and_then(|data| data.start_reading_session_by_slug_and_num)
        .ok_or_else(|| error("Ono did not return a reading session"))
}

fn unlock_wait_until_free(publication_id: &str) -> ExtensionResult<()> {
    let body = graphql_or_fixture(
        "unlockPublicationByWnR",
        UNLOCK_WNR_MUTATION,
        json!({ "publicationId": publication_id }),
        UNLOCK_FIXTURE,
    );
    let success = serde_json::from_str::<GraphQlResponse<UnlockData>>(&body)
        .ok()
        .and_then(|payload| payload.data)
        .and_then(|data| data.unlock_publication_by_wn_r)
        .is_some_and(|result| result.success == Some(true));
    if success {
        Ok(())
    } else {
        Err(error("Unable to unlock Ono wait-until-free chapter"))
    }
}

fn pages_from_session(
    payload: ReadingSessionPayload,
    slug: &str,
    num: &str,
) -> ExtensionResult<Vec<MangaPage>> {
    match payload.typename.as_str() {
        "SessionStarted" => {
            let pages = payload
                .publication_metadata
                .map(|metadata| metadata.pages)
                .unwrap_or_default();
            Ok(pages
                .into_iter()
                .enumerate()
                .map(|(index, image)| MangaPage {
                    content: PageContent::Url {
                        url: format!("{image}#{slug}/{num}"),
                        context: Some(manga::image_headers(BASE_URL)),
                    },
                    headers: manga::image_headers(BASE_URL),
                    description: Some(format!("Page {}", index + 1)),
                    ..MangaPage::default()
                })
                .collect())
        }
        "PublicationUnavailable" => Err(error(&format!(
            "Ono chapter unavailable: {}",
            payload
                .unavailability_reason
                .unwrap_or_else(|| "unknown".into())
        ))),
        "UserHasNotAccess" => {
            if payload
                .publication_access_methods
                .iter()
                .any(|method| method.typename == "NotLoggedIn")
            {
                Err(error(
                    "Ono login required. Open the Ono login preference to establish a session.",
                ))
            } else {
                Err(error("Ono chapter requires premium access"))
            }
        }
        other => Err(error(&format!("Ono reading access denied: {other}"))),
    }
}

fn pref_bool(request: &Value, id: &str, default: bool) -> bool {
    request
        .get("preferences")
        .and_then(|prefs| prefs.get(id))
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn normalize_key(input: &str) -> String {
    let value = input
        .strip_prefix(BASE_URL)
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim_matches('/');
    let mut parts = value.split('/');
    let kind = parts.next().unwrap_or("manga");
    let slug = parts.next().unwrap_or("sample");
    format!("/{kind}/{slug}")
}

fn absolute_url(key: &str) -> String {
    format!("{BASE_URL}/{}", key.trim_start_matches('/'))
}

fn sample_key() -> String {
    "/manga/sample".to_string()
}

fn fallback_item(key: &str) -> CatalogItem {
    CatalogItem {
        key: key.to_string(),
        title: url::slug_from_url(key).unwrap_or_else(|| "Ono".to_string()),
        url: Some(absolute_url(key)),
        language: Some("fr".to_string()),
        content_rating: Some("adult".to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn sample_series() -> SeriesDetail {
    serde_json::from_str(SERIES_DETAIL_JSON).unwrap_or_default()
}

fn slug_num_from_chapter_key(key: &str) -> (String, String) {
    let mut parts = key.trim_matches('/').split('/');
    let _kind = parts.next();
    let slug = parts.next().unwrap_or("sample").to_string();
    let num = parts.next().unwrap_or("1").to_string();
    (slug, num)
}

fn content_path(content_type: &str) -> &'static str {
    if content_type.eq_ignore_ascii_case("MANGA") {
        "manga"
    } else {
        "webtoon"
    }
}

fn parse_status(status: Option<&str>) -> ItemStatus {
    match status
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase()
        .as_str()
    {
        "ONGOING" => ItemStatus::Ongoing,
        "FINISHED" => ItemStatus::Completed,
        "HIATUS" => ItemStatus::Hiatus,
        "UNPUBLISHED" => ItemStatus::Cancelled,
        _ => ItemStatus::Unknown,
    }
}

fn extract_value_with_keys(body: &str, keys: &[&str]) -> Option<Value> {
    let bytes = body.as_bytes();
    let first_key = keys.first()?;
    for (index, _) in body.match_indices(&format!("\"{first_key}\"")) {
        if let Some(start) = body[..index].rfind('{') {
            if let Some(end) = matching_brace(bytes, start) {
                let candidate = &body[start..=end];
                if keys
                    .iter()
                    .all(|key| candidate.contains(&format!("\"{key}\"")))
                {
                    if let Ok(value) = serde_json::from_str(candidate) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

fn matching_brace(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, &byte) in bytes[start..].iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(start + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn error(message: &str) -> ExtensionError {
    ExtensionError {
        message: message.to_string(),
    }
}

#[derive(Debug, Default, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankingData {
    get_catalog_ranking: Option<RankingPayload>,
}

#[derive(Debug, Default, Deserialize)]
struct RankingPayload {
    #[serde(default)]
    series: Vec<RankingSeries>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RankingSeries {
    id: String,
    slug: String,
    title: String,
    content_type: String,
    #[serde(rename = "imageURL")]
    image_url: Option<String>,
}

impl RankingSeries {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/{}/{}", content_path(&self.content_type), self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.image_url.or_else(|| {
                Some(format!(
                    "https://catalog.ono.live/master/contents/{}/thumbnail",
                    self.id
                ))
            }),
            url: Some(absolute_url(&key)),
            language: Some("fr".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchCatalogData {
    search_catalog_by_term: Option<SearchCatalogResult>,
}

#[derive(Debug, Default, Deserialize)]
struct SearchCatalogResult {
    #[serde(default)]
    series: Vec<SearchSeries>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchSeries {
    id: String,
    title: String,
    slug: String,
    content_type: String,
}

impl SearchSeries {
    fn into_catalog(self) -> CatalogItem {
        let key = format!("/{}/{}", content_path(&self.content_type), self.slug);
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: Some(format!(
                "https://catalog.ono.live/master/contents/{}/thumbnail",
                self.id
            )),
            url: Some(absolute_url(&key)),
            language: Some("fr".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesDetail {
    slug: String,
    title: String,
    content_type: String,
    #[serde(default)]
    series_elements: Vec<SeriesElement>,
    summary: Option<String>,
    punchline: Option<String>,
    publication_status: Option<String>,
    cover: Option<String>,
    #[serde(default)]
    contributors: Vec<Contributor>,
    #[serde(default)]
    genres: Vec<Label>,
    #[serde(default)]
    tags: Vec<Label>,
}

impl SeriesDetail {
    fn key(&self) -> String {
        format!("/{}/{}", content_path(&self.content_type), self.slug)
    }

    fn into_catalog(self) -> CatalogItem {
        let key = self.key();
        let authors = self
            .contributors
            .iter()
            .map(|contributor| contributor.name.clone())
            .collect::<Vec<_>>();
        let mut tags = self
            .genres
            .into_iter()
            .chain(self.tags)
            .map(|label| label.label)
            .filter(|label| !label.trim().is_empty())
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.cover,
            authors: authors.clone(),
            artists: authors,
            description: [self.punchline, self.summary]
                .into_iter()
                .flatten()
                .filter(|value| !value.trim().is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
                .into(),
            tags,
            status: parse_status(self.publication_status.as_deref()),
            url: Some(absolute_url(&key)),
            language: Some("fr".to_string()),
            content_rating: Some("adult".to_string()),
            initialized: true,
            ..CatalogItem::default()
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct Contributor {
    name: String,
}

#[derive(Debug, Default, Deserialize)]
struct Label {
    label: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SeriesElement {
    num: String,
    title: Option<String>,
    price: Option<String>,
    is_bought: Option<bool>,
    wait_and_read: Option<WaitAndRead>,
}

#[derive(Debug, Default, Deserialize)]
struct WaitAndRead {}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartReadingSessionData {
    start_reading_session_by_slug_and_num: Option<ReadingSessionPayload>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadingSessionPayload {
    #[serde(rename = "__typename")]
    typename: String,
    unavailability_reason: Option<String>,
    publication_metadata: Option<PublicationMetadata>,
    #[serde(default)]
    publication_access_methods: Vec<AccessMethod>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccessMethod {
    #[serde(rename = "__typename")]
    typename: String,
    publication_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UnlockData {
    unlock_publication_by_wn_r: Option<UnlockResult>,
}

#[derive(Debug, Default, Deserialize)]
struct UnlockResult {
    success: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
struct PublicationMetadata {
    #[serde(default)]
    pages: Vec<String>,
}

const SEARCH_QUERY: &str = r#"query searchCatalogByTerm($term:String!){searchCatalogByTerm(input:{term:$term}){series{id title contentType slug}}}"#;
const RANKING_QUERY: &str = r#"query getCatalogRanking($genreSlug:String){getCatalogRanking(filter:{genreSlug:$genreSlug}){__typename ...on GetCatalogRankingPayload{series{id slug title contentType imageURL}}...on ErrorWithCode{__typename code}}}"#;
const START_READING_QUERY: &str = r#"query StartReadingSession($num:String!,$slug:String!){startReadingSessionBySlugAndNum(input:{num:$num slug:$slug}){...C}}fragment C on StartReadingSessionPayload{...on PublicationUnavailable{__typename unavailabilityReason}...on UserHasNotAccess{__typename publicationAccessMethods{...A}}...on ErrorWithCode{__typename code}...on SessionStarted{__typename publicationMetadata{pages}}}fragment A on PublicationAccessMethod{__typename ...on WaitNReadIsUsed{waitAndReadReloadDelay}...on WaitNReadAvailable{publicationId}...on CanBeBought{publicationId}...on NotEnoughCoins{publicationId}...on GiftTicketsAvailable{publicationId}}"#;
const UNLOCK_WNR_MUTATION: &str = r#"mutation unlockPublicationByWnR($publicationId:UUID!){unlockPublicationByWnR(input:{publicationId:$publicationId}){...on UnlockPublicationResult{success}...on ErrorWithCode{__typename code}}}"#;

const RANKING_FIXTURE: &str = r#"{"data":{"getCatalogRanking":{"series":[{"id":"1","slug":"sample","title":"Sample Ono","contentType":"MANGA","imageURL":"https://catalog.ono.live/master/contents/1/thumbnail"}]}}}"#;
const SEARCH_FIXTURE: &str = r#"{"data":{"searchCatalogByTerm":{"series":[{"id":"1","title":"Sample Ono","slug":"sample","contentType":"MANGA"}]}}}"#;
const SERIES_DETAIL_JSON: &str = r#"{"id":"1","slug":"sample","title":"Sample Ono","contentType":"MANGA","seriesElements":[{"id":"e1","num":"1","title":"Episode 1","price":"0","isBought":true},{"id":"e2","num":"2","title":"Episode 2","price":"10","isBought":false,"waitAndRead":{"__typename":"WaitNReadAvailable"}}],"summary":"Summary","punchline":"Punchline","publicationStatus":"ONGOING","cover":"https://catalog.ono.live/master/contents/1/thumbnail","contributors":[{"name":"Creator"}],"genres":[{"label":"Action"}],"tags":[{"label":"Romance"}]}"#;
const DETAILS_FIXTURE: &str = SERIES_DETAIL_JSON;
const READING_FIXTURE: &str = r#"{"data":{"startReadingSessionBySlugAndNum":{"__typename":"SessionStarted","publicationMetadata":{"pages":["https://cdn.ono.live/page1.jpg","https://cdn.ono.live/page2.jpg"]}}}}"#;
const UNLOCK_FIXTURE: &str = r#"{"data":{"unlockPublicationByWnR":{"success":true}}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_ranking_and_search() {
        assert_eq!(
            parse_ranking(RANKING_FIXTURE).entries[0].title,
            "Sample Ono"
        );
        assert_eq!(parse_search(SEARCH_FIXTURE).entries[0].key, "/manga/sample");
    }

    #[test]
    fn parses_details_chapters_and_pages() {
        let details = parse_series_detail(DETAILS_FIXTURE).unwrap();
        assert_eq!(details.title, "Sample Ono");
        let chapters = chapters_from_series(&details, false, true);
        assert_eq!(chapters.len(), 2);
        let pages = SOURCE.pages(json!({"chapter":"/manga/sample/1"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}

export_manga_source!(SOURCE);
