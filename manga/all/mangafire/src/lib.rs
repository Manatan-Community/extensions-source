use std::collections::{BTreeMap, BTreeSet};

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine as _,
};
use manatan_sdk::{
    client::{BrowserChallengePolicy, Client},
    context, CatalogItem, Error, FilterDefinition, MangaChapter, MangaPage, MangaSource,
    OptionItem, PageContent, Paged, PreferenceDefinition, Result, UrlResolveResult,
};
use scraper::Html;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

const BASE_URL: &str = "https://mangafire.to";
const REFERER: &str = "https://mangafire.to/";
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";
const REQUEST_LIMIT_MS: u32 = 500;
const PAGE_SIZE: u32 = 50;
const CHAPTER_PAGE_SIZE: u32 = 200;
const LANGUAGE_PREFERENCE_KEY: &str = "chapter_language";
const DEFAULT_SORT: &str = "relevance:desc";
const DEFAULT_GENRE_MODE: &str = "and";
const PLAY_ALLOWED_CONTENT_RATINGS: [&str; 2] = ["safe", "suggestive"];
const PLAY_BLOCKED_GENRE_IDS: [&str; 5] = ["268929", "7", "268930", "268931", "268932"];
const PLAY_BLOCKED_LABELS: [&str; 8] = [
    "adult",
    "ecchi",
    "erotica",
    "explicit",
    "hentai",
    "mature",
    "pornographic",
    "smut",
];
const VRF_TABLE_1: &str = "yINlmUNho8VYJT+ibTIP+9ESiULpVEtMOoD6U6lRE0R/xwXo/Xp9NrUgC4cw/Lmo33vUyjUE40kUoEWIr/fxfNNcq2s79ShQ5NhNrFnJ4hXPwOu/SuXzIbuTQKGFvfm08E9jvCfqAtoDqvQq3dVWPQFmJjgvkISBeXY3BgANR+yVnjGbcxZ47d6kLNfZPIayTq3/YGySb1KuVZodWp/WGNAO5pfMcpaK53Hhs0allBszaMaxuouOwdxbwgxIw6YunSsXjI05Yi0j9j4eHKfSXR8Ifo/Od+8iamRfCXTyvm7NGRGYdcQ0ywcK/u6RXhrbcCm4t2eCtrDgQVecJGkQ+A==";
const VRF_KEY_1: &str = "0Ec58JOY3uBzJK9m3zqIOpdlF7UFiax9DmA=";
const VRF_TABLE_2: &str = "IUFltCxD3Oc2cwCgkJffthaOg9cgPUb0LgW6H/VtfcF0kc5F25t+aWj6JH9VOhOaY0rAFdUxlDnl5BLNvwEJvQtP5qcw7vdb/K+chnbwnspSHT8mz5lqwz41TezG0hkO06FTjJZhsyNuFLDpD2ZZxQj/QIRcF90zpmQ7Byu483WsQqUE0C342HL+JXngRB6fRzxRyVTaKu83h7UYTJ0QMt6ixFh6S3F8gqkKwrGTL3jHNBsD45UnifK8+RGtishQV2K3rujLKEkiZxpr2dYcudFW4oFsDKhad3CLBvuyTqsCo4B7mL5IKQ1vXo/MOOvq1I1d8ar9X6Ttu5KF4fZgiA==";
const VRF_KEY_2: &str = "AAdjb1iPY8CiDmq9H34tKTBF8a3oDQ==";
const VRF_TABLE_3: &str = "NQHlu1/wVO5EmkwQymF810qqY2xG1k2obcas4Z9mCsPEIFl9pRIjFxbJ7ybMHbBckT5Ton85E0FOeHezbh/mjlEYpmpnlXOS8dgrqeq2KfxImTh1YK9y0PeMNhzA1OQzSY9brYOJq/l2QnE/hwOeZIhPixVSKIUlDb5vLcH6RWKxkIEMuP0bDwIqQ71AJJaEaMJL7A6YtyIwoRT+L5v4aZzodN/0+3nOGsfblFjgxSfPzVDjNFeNl5P26+kEC/8AHgdrpAbt3hHz3HrRN1Y6e+JHgF7ncFWnoF0y3THL1S71WgWGCa6KtSzTCCG58n68nTyj2T3Sshk7utqCtMi/ZQ==";
const VRF_KEY_3: &str = "DELOJgPsVaCcblDtTGMdHzM=";

pub struct MangaFireSource {
    client: Client,
    challenge: BrowserChallengePolicy,
}

impl Default for MangaFireSource {
    fn default() -> Self {
        Self {
            client: Client::browser()
                .cookies_for(BASE_URL)
                .header("Referer", REFERER)
                .header("Accept", "application/json"),
            challenge: BrowserChallengePolicy::cloudflare(BASE_URL).profile("mangafire-cloudflare"),
        }
    }
}

impl MangaFireSource {
    fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let url = signed_api_url(url)?;
        let response = self
            .client
            .get(&url)
            .rate_limit("mangafire", REQUEST_LIMIT_MS)
            .send_with_challenge(&self.challenge)?
            .error_for_status()?;
        let body = response.text()?;
        serde_json::from_str(body).map_err(json_error)
    }

    fn selected_language(&self) -> LanguageVariant {
        let preferred = context::preference::<String>(LANGUAGE_PREFERENCE_KEY)
            .ok()
            .flatten()
            .unwrap_or_else(|| LanguageVariant::default().source_code.to_owned());
        LanguageVariant::from_source_code(&preferred)
    }

    fn filtered_catalog_page(payload: ApiResponse<MangaDto>) -> Paged<CatalogItem> {
        let has_next_page = payload.meta.map(|meta| meta.has_next).unwrap_or(false);
        Paged {
            entries: payload
                .items
                .into_iter()
                .map(MangaDto::into_catalog_item)
                .collect(),
            has_next_page,
        }
    }

    fn ensure_safe_title(&self, item_key_or_url: &str) -> Result<MangaDetailsDto> {
        let hid = title_hid(item_key_or_url)?;
        let payload: MangaDetailsResponse =
            self.get_json(&format!("{BASE_URL}/api/titles/{hid}"))?;
        payload.data.ensure_play_allowed()?;
        Ok(payload.data)
    }
}

impl MangaSource for MangaFireSource {
    fn popular(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = listing_url("views_30d", "desc", page)?;
        let payload: ApiResponse<MangaDto> = self.get_json(&url)?;
        Ok(Self::filtered_catalog_page(payload))
    }

    fn latest(&mut self, page: u32) -> Result<Paged<CatalogItem>> {
        let url = listing_url("chapter_updated_at", "desc", page)?;
        let payload: ApiResponse<MangaDto> = self.get_json(&url)?;
        Ok(Self::filtered_catalog_page(payload))
    }

    fn search(&mut self, query: &str, page: u32, filters: &Value) -> Result<Paged<CatalogItem>> {
        let author_id = match author_filter(filters) {
            Some(author_query) if !author_query.is_empty() => {
                let tags: TagResponse = self.get_json(&tag_lookup_url(author_query)?)?;
                let Some(author_id) = select_author_tag(&tags) else {
                    return Ok(Paged {
                        entries: Vec::new(),
                        has_next_page: false,
                    });
                };
                Some(author_id)
            }
            _ => None,
        };

        let url = search_url(query, page, filters, author_id.as_deref())?;
        let payload: ApiResponse<MangaDto> = self.get_json(&url)?;
        Ok(Self::filtered_catalog_page(payload))
    }

    fn details(&mut self, item: CatalogItem) -> Result<CatalogItem> {
        let details = self.ensure_safe_title(item.url.as_deref().unwrap_or(&item.key))?;
        let mut parsed = details.into_catalog_item()?;
        parsed.key = item.key;
        parsed.url = item.url.or(Some(absolute_url(BASE_URL, &parsed.key)?));
        Ok(parsed)
    }

    fn chapters(&mut self, item: CatalogItem) -> Result<Vec<MangaChapter>> {
        let manga_path = title_path(item.url.as_deref().unwrap_or(&item.key))?;
        self.ensure_safe_title(&manga_path)?;
        let language = self.selected_language();
        let mut page = 1;
        let mut source_order = 0_i32;
        let mut chapters = Vec::new();

        loop {
            let url = chapters_url(&manga_path, language.api_code, page)?;
            let payload: ApiResponse<ChapterDto> = self.get_json(&url)?;
            let last_page = payload
                .meta
                .as_ref()
                .map(|meta| meta.last_page)
                .unwrap_or(1)
                .max(1);
            for chapter in payload.items {
                chapters.push(chapter.into_manga_chapter(&manga_path, language, source_order)?);
                source_order += 1;
            }
            if page >= last_page {
                break;
            }
            page += 1;
        }

        Ok(chapters)
    }

    fn pages(&mut self, item: CatalogItem, chapter: MangaChapter) -> Result<Vec<MangaPage>> {
        self.ensure_safe_title(item.url.as_deref().unwrap_or(&item.key))?;
        let chapter_id = chapter_id(&chapter)?;
        let payload: PagesResponse =
            self.get_json(&format!("{BASE_URL}/api/chapters/{chapter_id}"))?;
        payload
            .data
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| page.into_manga_page(index))
            .collect()
    }

    fn filters(&mut self) -> Result<Vec<FilterDefinition>> {
        Ok(filters())
    }

    fn preferences(&mut self) -> Result<Vec<PreferenceDefinition>> {
        Ok(preferences())
    }

    fn item_url(&mut self, item: &CatalogItem) -> Result<Option<String>> {
        Ok(Some(absolute_url(
            BASE_URL,
            item.url.as_deref().unwrap_or(&item.key),
        )?))
    }

    fn chapter_url(
        &mut self,
        _item: &CatalogItem,
        chapter: &MangaChapter,
    ) -> Result<Option<String>> {
        Ok(Some(absolute_url(
            BASE_URL,
            chapter.url.as_deref().unwrap_or(&chapter.key),
        )?))
    }

    fn handle_url(&mut self, candidate: &str) -> Result<Option<UrlResolveResult>> {
        let base = Url::parse(BASE_URL).map_err(url_error)?;
        let url = Url::parse(candidate).map_err(url_error)?;
        if base.scheme() != url.scheme()
            || base.host_str() != url.host_str()
            || base.port_or_known_default() != url.port_or_known_default()
            || !url.path().starts_with("/title/")
        {
            return Ok(None);
        }

        let item_path = item_path_from_candidate(url.path())?;
        let safe_item = self.ensure_safe_title(&item_path)?.into_catalog_item()?;
        let item_url = absolute_url(BASE_URL, &item_path)?;
        let mut result = UrlResolveResult {
            item: Some(CatalogItem {
                url: Some(item_url),
                ..safe_item
            }),
            ..UrlResolveResult::default()
        };

        if let Some(chapter_path) = chapter_path_from_candidate(url.path()) {
            let chapter_number = chapter_number_from_path(&chapter_path);
            let api_lang = chapter_api_language_from_path(&chapter_path)
                .unwrap_or_else(|| LanguageVariant::default().api_code.to_owned());
            result.chapter_key = Some(chapter_path.clone());
            result.manga_chapter = Some(MangaChapter {
                key: chapter_path.clone(),
                url: Some(absolute_url(BASE_URL, &chapter_path)?),
                chapter_number,
                language: Some(
                    LanguageVariant::from_api_code(&api_lang)
                        .source_code
                        .to_owned(),
                ),
                extra: chapter_extra(chapter_id_from_path(&chapter_path)?, &api_lang),
                ..MangaChapter::default()
            });
        }

        Ok(Some(result))
    }
}

#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(
    manatan_sdk::Extension::new().manga("mangafire", MangaFireSource::default())
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LanguageVariant {
    source_code: &'static str,
    api_code: &'static str,
    label: &'static str,
}

impl Default for LanguageVariant {
    fn default() -> Self {
        Self::from_source_code("en")
    }
}

impl LanguageVariant {
    const ALL: [Self; 7] = [
        Self {
            source_code: "en",
            api_code: "en",
            label: "English",
        },
        Self {
            source_code: "es",
            api_code: "es",
            label: "Spanish",
        },
        Self {
            source_code: "es-419",
            api_code: "es-la",
            label: "Spanish (Latin America)",
        },
        Self {
            source_code: "fr",
            api_code: "fr",
            label: "French",
        },
        Self {
            source_code: "ja",
            api_code: "ja",
            label: "Japanese",
        },
        Self {
            source_code: "pt",
            api_code: "pt",
            label: "Portuguese",
        },
        Self {
            source_code: "pt-BR",
            api_code: "pt-br",
            label: "Portuguese (Brazil)",
        },
    ];

    fn from_source_code(code: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|variant| variant.source_code.eq_ignore_ascii_case(code))
            .unwrap_or(Self::ALL[0])
    }

    fn from_api_code(code: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|variant| variant.api_code.eq_ignore_ascii_case(code))
            .unwrap_or(Self::ALL[0])
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiResponse<T> {
    #[serde(default)]
    items: Vec<T>,
    meta: Option<ApiMeta>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiMeta {
    #[serde(default = "one")]
    last_page: u32,
    #[serde(default)]
    has_next: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct PosterDto {
    small: Option<String>,
    medium: Option<String>,
    large: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct MangaDto {
    hid: String,
    slug: Option<String>,
    title: String,
    poster: Option<PosterDto>,
}

impl MangaDto {
    fn into_catalog_item(self) -> CatalogItem {
        let key = manga_path(&self.hid, self.slug.as_deref());
        CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.poster.and_then(select_poster).map(Into::into),
            url: absolute_url(BASE_URL, &key).ok(),
            language: Some("all".to_owned()),
            initialized: false,
            ..CatalogItem::default()
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDetailsResponse {
    data: MangaDetailsDto,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MangaDetailsDto {
    hid: String,
    slug: Option<String>,
    title: String,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    rating: Option<f32>,
    #[serde(default)]
    content_rating: Option<String>,
    poster: Option<PosterDto>,
    #[serde(default)]
    synopsis_html: Option<String>,
    #[serde(default)]
    alt_titles: Vec<String>,
    #[serde(default)]
    authors: Vec<EntityDto>,
    #[serde(default)]
    artists: Vec<EntityDto>,
    #[serde(default)]
    genres: Vec<EntityDto>,
    #[serde(default)]
    themes: Vec<EntityDto>,
}

impl MangaDetailsDto {
    fn play_content_rating(&self) -> Result<String> {
        let content_rating = self
            .content_rating
            .as_deref()
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| Error::new("MangaFire title has no content classification"))?;
        if !PLAY_ALLOWED_CONTENT_RATINGS.contains(&content_rating.as_str()) {
            return Err(Error::new("MangaFire title is not available in this build"));
        }
        let blocked = self
            .genres
            .iter()
            .chain(self.themes.iter())
            .map(|entry| entry.title.trim().to_ascii_lowercase())
            .any(|label| PLAY_BLOCKED_LABELS.contains(&label.as_str()));
        if blocked {
            return Err(Error::new("MangaFire title is not available in this build"));
        }
        Ok(content_rating)
    }

    fn ensure_play_allowed(&self) -> Result<()> {
        self.play_content_rating().map(|_| ())
    }

    fn into_catalog_item(self) -> Result<CatalogItem> {
        self.ensure_play_allowed()?;
        let key = manga_path(&self.hid, self.slug.as_deref());
        let mut tags = Vec::new();
        if let Some(kind) = self.r#type.as_deref() {
            tags.push(title_case(kind));
        }
        tags.extend(self.genres.iter().map(|entry| entry.title.clone()));
        tags.extend(self.themes.iter().map(|entry| entry.title.clone()));
        dedupe(&mut tags);

        let mut item = CatalogItem {
            key: key.clone(),
            title: self.title,
            cover: self.poster.and_then(select_poster).map(Into::into),
            url: Some(absolute_url(BASE_URL, &key)?),
            authors: self.authors.into_iter().map(|entry| entry.title).collect(),
            artists: self.artists.into_iter().map(|entry| entry.title).collect(),
            description: self.synopsis_html.as_deref().map(html_to_text),
            tags,
            language: Some("all".to_owned()),
            rating: self.rating,
            content_rating: self.content_rating,
            status: Some(json!(status_from_api(self.status.as_deref()))),
            initialized: true,
            ..CatalogItem::default()
        };
        let mut alternate_titles = self.alt_titles;
        dedupe(&mut alternate_titles);
        if !alternate_titles.is_empty() {
            item.extra
                .insert("aliases".to_owned(), json!(alternate_titles));
        }
        Ok(item)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct EntityDto {
    title: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TagResponse {
    #[serde(default)]
    data: Vec<TagDto>,
}

#[derive(Clone, Debug, Deserialize)]
struct TagDto {
    id: u64,
    #[serde(rename = "type")]
    tag_type: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChapterDto {
    id: u64,
    number: f32,
    #[serde(default)]
    name: Option<String>,
    language: String,
    #[serde(default)]
    created_at: Option<i64>,
}

impl ChapterDto {
    fn into_manga_chapter(
        self,
        manga_path: &str,
        selected_language: LanguageVariant,
        source_order: i32,
    ) -> Result<MangaChapter> {
        let api_language = if self.language.trim().is_empty() {
            selected_language.api_code
        } else {
            self.language.as_str()
        };
        let source_language = LanguageVariant::from_api_code(api_language).source_code;
        let number = chapter_number_string(self.number);
        let chapter_path = format!(
            "{manga_path}/{}-chapter-{}-{}",
            self.id, number, api_language
        );
        let title = match self.name.as_deref().map(str::trim) {
            Some(name) if !name.is_empty() => Some(format!("Ch. {number} - {name}")),
            _ => Some(format!("Ch. {number}")),
        };
        Ok(MangaChapter {
            key: chapter_path.clone(),
            title,
            chapter_number: Some(self.number),
            date_uploaded: self.created_at.map(|value| value * 1000),
            language: Some(source_language.to_owned()),
            url: Some(absolute_url(BASE_URL, &chapter_path)?),
            source_order: Some(source_order),
            extra: chapter_extra(self.id, api_language),
            ..MangaChapter::default()
        })
    }
}

#[derive(Clone, Debug, Deserialize)]
struct PagesResponse {
    data: ChapterDataDto,
}

#[derive(Clone, Debug, Deserialize)]
struct ChapterDataDto {
    #[serde(default)]
    pages: Vec<PageDto>,
}

#[derive(Clone, Debug, Deserialize)]
struct PageDto {
    url: String,
}

impl PageDto {
    fn into_manga_page(self, index: usize) -> Result<MangaPage> {
        let context = BTreeMap::from([
            ("Accept".to_owned(), "application/json".to_owned()),
            ("Referer".to_owned(), REFERER.to_owned()),
            ("User-Agent".to_owned(), BROWSER_USER_AGENT.to_owned()),
        ]);
        Ok(MangaPage {
            content: PageContent::Url {
                url: self.url,
                context: Some(context),
            },
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
    }
}

fn filters() -> Vec<FilterDefinition> {
    vec![
        check_group(
            "types",
            "Type",
            &[
                ("Manga", "manga"),
                ("Manhwa", "manhwa"),
                ("Manhua", "manhua"),
                ("Other", "other"),
            ],
        ),
        FilterDefinition::Separator,
        FilterDefinition::Select {
            id: "genres_mode".to_owned(),
            name: "Genre match mode".to_owned(),
            options: vec![
                OptionItem {
                    label: "AND".to_owned(),
                    value: "and".to_owned(),
                },
                OptionItem {
                    label: "OR".to_owned(),
                    value: "or".to_owned(),
                },
            ],
            default_index: 0,
        },
        tri_state_group(
            "genres",
            "Genres",
            &[
                ("Action", "1"),
                ("Adventure", "78"),
                ("Avant Garde", "3"),
                ("Boys Love", "4"),
                ("Comedy", "5"),
                ("Demons", "77"),
                ("Drama", "6"),
                ("Fantasy", "79"),
                ("Girls Love", "9"),
                ("Gourmet", "10"),
                ("Harem", "11"),
                ("Horror", "530"),
                ("Isekai", "13"),
                ("Iyashikei", "531"),
                ("Josei", "15"),
                ("Kids", "532"),
                ("Magic", "539"),
                ("Mahou Shoujo", "533"),
                ("Martial Arts", "534"),
                ("Mecha", "19"),
                ("Military", "535"),
                ("Music", "21"),
                ("Mystery", "22"),
                ("Parody", "23"),
                ("Psychological", "536"),
                ("Reverse Harem", "25"),
                ("Romance", "26"),
                ("School", "73"),
                ("Sci-Fi", "28"),
                ("Seinen", "537"),
                ("Shoujo", "30"),
                ("Shounen", "31"),
                ("Slice of Life", "538"),
                ("Space", "33"),
                ("Sports", "34"),
                ("Super Power", "75"),
                ("Supernatural", "76"),
                ("Suspense", "37"),
                ("Thriller", "38"),
                ("Vampire", "39"),
            ],
        ),
        FilterDefinition::Separator,
        check_group(
            "statuses",
            "Status",
            &[
                ("Releasing", "releasing"),
                ("Finished", "finished"),
                ("On Hiatus", "on_hiatus"),
                ("Discontinued", "discontinued"),
                ("Not Yet Released", "not_yet_released"),
            ],
        ),
        FilterDefinition::Separator,
        FilterDefinition::Text {
            id: "author".to_owned(),
            name: "Author / Artist".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "year_from".to_owned(),
            name: "Release year (From)".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "year_to".to_owned(),
            name: "Release year (To)".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Text {
            id: "min_chap".to_owned(),
            name: "Minimum chapters".to_owned(),
            default: String::new(),
        },
        FilterDefinition::Separator,
        FilterDefinition::Select {
            id: "sort".to_owned(),
            name: "Sort by".to_owned(),
            options: vec![
                option("Latest update", "chapter_updated_at:desc"),
                option("Best match", "relevance:desc"),
                option("Recently added", "created_at:desc"),
                option("Title (A-Z)", "title:asc"),
                option("Title (Z-A)", "title:desc"),
                option("Year (newest)", "year:desc"),
                option("Year (oldest)", "year:asc"),
                option("Highest rated", "score:desc"),
                option("Most viewed - 7 days", "views_7d:desc"),
                option("Most viewed - 30 days", "views_30d:desc"),
                option("Most viewed - all time", "views_total:desc"),
                option("Most followed", "follows_total:desc"),
            ],
            default_index: 1,
        },
    ]
}

fn preferences() -> Vec<PreferenceDefinition> {
    vec![PreferenceDefinition::Select {
        key: LANGUAGE_PREFERENCE_KEY.to_owned(),
        title: "Chapter language".to_owned(),
        options: LanguageVariant::ALL
            .into_iter()
            .map(|variant| OptionItem {
                label: variant.label.to_owned(),
                value: variant.source_code.to_owned(),
            })
            .collect(),
        default: "en".to_owned(),
    }]
}

fn listing_url(order_field: &str, direction: &str, page: u32) -> Result<String> {
    let mut url = Url::parse(&format!("{BASE_URL}/api/titles")).map_err(url_error)?;
    url.query_pairs_mut()
        .append_pair(&format!("order[{order_field}]"), direction)
        .append_pair("page", &page.max(1).to_string())
        .append_pair("limit", &PAGE_SIZE.to_string());
    append_play_safety_filters(&mut url);
    Ok(url.to_string())
}

fn search_url(query: &str, page: u32, filters: &Value, author_id: Option<&str>) -> Result<String> {
    let mut url = Url::parse(&format!("{BASE_URL}/api/titles")).map_err(url_error)?;
    let mut pairs = url.query_pairs_mut();
    let trimmed = query.trim();
    if !trimmed.is_empty() {
        pairs.append_pair("keyword", trimmed);
    }
    pairs
        .append_pair("page", &page.max(1).to_string())
        .append_pair("limit", &PAGE_SIZE.to_string())
        .append_pair(
            "genres_mode",
            select_value(filters, "genres_mode").unwrap_or(DEFAULT_GENRE_MODE),
        );

    if let Some(author_id) = author_id {
        pairs.append_pair("authors[]", author_id);
    }

    for value in group_values(filters, "types") {
        pairs.append_pair("types[]", &value);
    }

    let (genres_in, genres_ex) = tri_state_values(filters, "genres");
    for value in genres_in {
        pairs.append_pair("genres_in[]", &value);
    }
    for value in genres_ex {
        pairs.append_pair("genres_ex[]", &value);
    }
    for value in PLAY_BLOCKED_GENRE_IDS {
        pairs.append_pair("genres_ex[]", value);
    }
    for value in PLAY_ALLOWED_CONTENT_RATINGS {
        pairs.append_pair("content_rating[]", value);
    }

    for value in group_values(filters, "statuses") {
        pairs.append_pair("statuses[]", &value);
    }

    for (key, value) in [
        ("year_from", text_value(filters, "year_from")),
        ("year_to", text_value(filters, "year_to")),
        ("min_chap", positive_int_value(filters, "min_chap")),
    ] {
        if let Some(value) = value {
            pairs.append_pair(key, &value);
        }
    }

    let sort = select_value(filters, "sort").unwrap_or(DEFAULT_SORT);
    let (field, direction) = sort
        .split_once(':')
        .unwrap_or(("chapter_updated_at", "desc"));
    pairs.append_pair(&format!("order[{field}]"), direction);

    drop(pairs);
    Ok(url.to_string())
}

fn append_play_safety_filters(url: &mut Url) {
    let mut pairs = url.query_pairs_mut();
    for value in PLAY_BLOCKED_GENRE_IDS {
        pairs.append_pair("genres_ex[]", value);
    }
    for value in PLAY_ALLOWED_CONTENT_RATINGS {
        pairs.append_pair("content_rating[]", value);
    }
}

fn signed_api_url(candidate: &str) -> Result<String> {
    let mut url = Url::parse(candidate).map_err(url_error)?;
    let Some(path) = url.path().strip_prefix("/api") else {
        return Ok(candidate.to_owned());
    };

    let mut params = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    params.sort_by(|left, right| left.0.cmp(&right.0));

    let mut canonical = path.to_owned();
    if !params.is_empty() {
        canonical.push('?');
        let mut previous_array_key = "";
        let mut array_index = 0_usize;
        for (position, (key, value)) in params.iter().enumerate() {
            if position > 0 {
                canonical.push('&');
            }
            if let Some(base_key) = key.strip_suffix("[]") {
                if previous_array_key != key {
                    previous_array_key = key;
                    array_index = 0;
                }
                canonical.push_str(base_key);
                canonical.push('[');
                canonical.push_str(&array_index.to_string());
                canonical.push(']');
                array_index += 1;
            } else {
                canonical.push_str(key);
            }
            canonical.push('=');
            canonical.push_str(value);
        }
    }

    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        for (key, value) in params {
            query.append_pair(&key, &value);
        }
        query.append_pair("vrf", &vrf_sign(&canonical)?);
    }
    Ok(url.to_string())
}

fn vrf_sign(input: &str) -> Result<String> {
    let mut data = input.as_bytes().to_vec();
    for (table, key, iv) in [
        (VRF_TABLE_1, VRF_KEY_1, 0x5A),
        (VRF_TABLE_2, VRF_KEY_2, 0x35),
        (VRF_TABLE_3, VRF_KEY_3, 0xBA),
    ] {
        let table = STANDARD
            .decode(table)
            .map_err(|error| Error::new(format!("Invalid MangaFire VRF table: {error}")))?;
        let key = STANDARD
            .decode(key)
            .map_err(|error| Error::new(format!("Invalid MangaFire VRF key: {error}")))?;
        if table.len() != 256 || key.is_empty() {
            return Err(Error::new("Invalid MangaFire VRF stage"));
        }
        data = vrf_encrypt_stage(&data, &table, &key, iv);
    }
    Ok(URL_SAFE_NO_PAD.encode(data))
}

fn vrf_encrypt_stage(data: &[u8], table: &[u8], key: &[u8], iv: usize) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut previous = iv;
    for (index, byte) in data.iter().enumerate() {
        previous =
            table[(*byte as usize ^ key[index % key.len()] as usize ^ previous) & 0xFF] as usize;
        output.push(previous as u8);
    }
    output
}

fn tag_lookup_url(query: &str) -> Result<String> {
    let mut url = Url::parse(&format!("{BASE_URL}/api/tags")).map_err(url_error)?;
    url.query_pairs_mut().append_pair("keyword", query.trim());
    Ok(url.to_string())
}

fn chapters_url(manga_path: &str, language: &str, page: u32) -> Result<String> {
    let hid = title_hid(manga_path)?;
    let mut url =
        Url::parse(&format!("{BASE_URL}/api/titles/{hid}/chapters")).map_err(url_error)?;
    url.query_pairs_mut()
        .append_pair("language", language)
        .append_pair("sort", "number")
        .append_pair("order", "desc")
        .append_pair("page", &page.max(1).to_string())
        .append_pair("limit", &CHAPTER_PAGE_SIZE.to_string());
    Ok(url.to_string())
}

fn select_author_tag(payload: &TagResponse) -> Option<String> {
    payload
        .data
        .iter()
        .find(|entry| entry.tag_type == "author" || entry.tag_type == "artist")
        .map(|entry| entry.id.to_string())
}

fn select_poster(poster: PosterDto) -> Option<String> {
    poster.large.or(poster.medium).or(poster.small)
}

fn absolute_url(base: &str, candidate: &str) -> Result<String> {
    Url::parse(base)
        .and_then(|base| base.join(candidate))
        .map(|url| {
            let mut canonical = url;
            canonical.set_query(None);
            canonical.set_fragment(None);
            canonical.to_string()
        })
        .map_err(url_error)
}

fn title_hid(candidate: &str) -> Result<String> {
    let path = title_path(candidate)?;
    let segment = path
        .trim_end_matches('/')
        .split('/')
        .nth(2)
        .ok_or_else(|| Error::new("MangaFire title path is missing the hid segment"))?;
    Ok(segment
        .split(['.', '-'])
        .next()
        .unwrap_or_default()
        .to_owned())
}

fn title_path(candidate: &str) -> Result<String> {
    let path = parse_path(candidate)?;
    let mut segments = path
        .trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty());
    let Some(first) = segments.next() else {
        return Err(Error::new("MangaFire URL is missing a path"));
    };
    let Some(second) = segments.next() else {
        return Err(Error::new(
            "MangaFire title URL is missing its slug segment",
        ));
    };
    if first != "title" {
        return Err(Error::new("MangaFire URL does not point at a title"));
    }
    Ok(format!("/{first}/{second}"))
}

fn item_path_from_candidate(path: &str) -> Result<String> {
    title_path(path)
}

fn chapter_path_from_candidate(path: &str) -> Option<String> {
    let path = parse_path(path).ok()?;
    let parts: Vec<_> = path
        .trim_end_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
    (parts.len() >= 3 && parts[0] == "title").then(|| format!("/{}", parts.join("/")))
}

fn chapter_id(chapter: &MangaChapter) -> Result<u64> {
    if let Some(id) = chapter.extra.get("id").and_then(Value::as_u64) {
        return Ok(id);
    }
    chapter_id_from_path(chapter.url.as_deref().unwrap_or(&chapter.key))
}

fn chapter_id_from_path(candidate: &str) -> Result<u64> {
    let path = parse_path(candidate)?;
    let segment = path
        .trim_end_matches('/')
        .split('/')
        .next_back()
        .ok_or_else(|| Error::new("MangaFire chapter URL is missing its chapter segment"))?;
    let id = segment
        .split("-chapter-")
        .next()
        .unwrap_or_default()
        .parse::<u64>()
        .map_err(|error| Error::new(format!("Invalid MangaFire chapter id: {error}")))?;
    Ok(id)
}

fn chapter_number_from_path(candidate: &str) -> Option<f32> {
    let path = parse_path(candidate).ok()?;
    let segment = path.trim_end_matches('/').split('/').next_back()?;
    let after = segment.split("-chapter-").nth(1)?;
    let number = after.rsplit_once('-')?.0;
    number.parse::<f32>().ok()
}

fn chapter_api_language_from_path(candidate: &str) -> Option<String> {
    let path = parse_path(candidate).ok()?;
    let segment = path.trim_end_matches('/').split('/').next_back()?;
    segment
        .rsplit_once('-')
        .map(|(_, language)| language.to_owned())
}

fn chapter_extra(id: u64, api_language: &str) -> std::collections::BTreeMap<String, Value> {
    [
        ("id".to_owned(), Value::from(id)),
        ("apiLanguage".to_owned(), Value::from(api_language)),
    ]
    .into_iter()
    .collect()
}

fn parse_path(candidate: &str) -> Result<String> {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        let url = Url::parse(candidate).map_err(url_error)?;
        Ok(url.path().to_owned())
    } else {
        Ok(candidate.to_owned())
    }
}

fn manga_path(hid: &str, slug: Option<&str>) -> String {
    match slug.filter(|value| !value.trim().is_empty()) {
        Some(slug) => format!("/title/{hid}-{slug}"),
        None => format!("/title/{hid}"),
    }
}

fn chapter_number_string(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        let rendered = value.to_string();
        rendered
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

fn html_to_text(value: &str) -> String {
    let fragment = Html::parse_fragment(value);
    normalize_space(&fragment.root_element().text().collect::<Vec<_>>().join(" "))
}

fn normalize_space(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn status_from_api(value: Option<&str>) -> &'static str {
    match value.unwrap_or_default().to_ascii_lowercase().as_str() {
        "releasing" => "ongoing",
        "finished" => "completed",
        "on_hiatus" => "hiatus",
        "discontinued" => "cancelled",
        _ => "unknown",
    }
}

fn option(label: &str, value: &str) -> OptionItem {
    OptionItem {
        label: label.to_owned(),
        value: value.to_owned(),
    }
}

fn check_group(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Group {
        id: id.to_owned(),
        name: name.to_owned(),
        filters: values
            .iter()
            .map(|(label, value)| FilterDefinition::CheckBox {
                id: (*value).to_owned(),
                name: (*label).to_owned(),
                default: false,
            })
            .collect(),
    }
}

fn tri_state_group(id: &str, name: &str, values: &[(&str, &str)]) -> FilterDefinition {
    FilterDefinition::Group {
        id: id.to_owned(),
        name: name.to_owned(),
        filters: values
            .iter()
            .map(|(label, value)| FilterDefinition::TriState {
                id: (*value).to_owned(),
                name: (*label).to_owned(),
                default: 0,
            })
            .collect(),
    }
}

fn text_value(filters: &Value, key: &str) -> Option<String> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn positive_int_value(filters: &Value, key: &str) -> Option<String> {
    text_value(filters, key).and_then(|value| {
        value
            .parse::<u32>()
            .ok()
            .filter(|number| *number > 0)
            .map(|number| number.to_string())
    })
}

fn select_value<'a>(filters: &'a Value, key: &str) -> Option<&'a str> {
    filters
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn author_filter(filters: &Value) -> Option<&str> {
    filters.get("author").and_then(Value::as_str).map(str::trim)
}

fn group_values(filters: &Value, key: &str) -> Vec<String> {
    match filters.get(key) {
        Some(Value::Object(entries)) => entries
            .iter()
            .filter(|(_, selected)| selected.as_bool().unwrap_or(false))
            .map(|(entry, _)| entry.clone())
            .collect(),
        Some(Value::Array(entries)) => entries
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn tri_state_values(filters: &Value, key: &str) -> (Vec<String>, Vec<String>) {
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    if let Some(Value::Object(entries)) = filters.get(key) {
        for (entry, state) in entries {
            match state {
                Value::Bool(true) => include.push(entry.clone()),
                Value::Bool(false) => exclude.push(entry.clone()),
                _ => {}
            }
        }
    }
    (include, exclude)
}

fn one() -> u32 {
    1
}

fn json_error(error: serde_json::Error) -> Error {
    Error::new(format!("MangaFire JSON parse error: {error}"))
}

fn url_error(error: url::ParseError) -> Error {
    Error::new(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    const MANIFEST: &str = include_str!("../manifest.json");
    const ICON: &[u8] = include_bytes!("../assets/icon.png");
    const ICON_SHA256: &str = "b1543e7b1238f29306ba24e9f48e0046dc6c47fc157c3703650bed73f16acf8e";
    const POPULAR_FIXTURE: &str = include_str!("../fixtures/popular.json");
    const DETAILS_FIXTURE: &str = include_str!("../fixtures/details.json");
    const TAGS_FIXTURE: &str = include_str!("../fixtures/tags.json");
    const CHAPTERS_PAGE_1_FIXTURE: &str = include_str!("../fixtures/chapters-page-1.json");
    const CHAPTERS_PAGE_2_FIXTURE: &str = include_str!("../fixtures/chapters-page-2.json");
    const PAGES_FIXTURE: &str = include_str!("../fixtures/pages.json");

    #[test]
    fn metadata_matches_expected_configuration() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["id"], "mangafire");
        assert_eq!(manifest["contentType"], "manga");
        assert_eq!(manifest["license"], "Apache-2.0");
        assert_eq!(manifest["permissions"]["cookies"], true);
        assert_eq!(manifest["permissions"]["webview"], true);
        assert_eq!(manifest["permissions"]["javascript"], false);
        assert_eq!(
            manifest["permissions"]["network"]["allow"],
            json!([
                "https://mangafire.to",
                "https://static.mfcdn.nl",
                "https://k99.mfcdn1.xyz",
                "https://k99.mfcdn2.xyz",
                "https://l1n.mfcdn3.xyz",
                "https://m3z.mfcdn2.xyz",
                "https://m3z.mfcdn3.xyz",
                "https://nw8.mfcdn1.xyz",
                "https://nw8.mfcdn3.xyz",
                "https://o48.mfcdn1.xyz",
                "https://o48.mfcdn2.xyz"
            ])
        );
        assert_eq!(
            manifest["sources"],
            json!([{
                "id": "mangafire",
                "name": "MangaFire",
                "lang": "all",
                "contentType": "manga",
                "baseUrl": "https://mangafire.to",
                "contentRating": "suggestive",
                "capabilities": {
                    "search": true,
                    "latest": true,
                    "filters": true,
                    "preferences": true,
                    "urlResolution": true
                },
                "listings": [
                    { "id": "popular", "name": "Popular" },
                    { "id": "latest", "name": "Latest" }
                ],
                "urlPatterns": [{ "pattern": "https://mangafire.to/title/*", "kind": "item-or-chapter" }],
                "tags": ["json-api"]
            }])
        );
    }

    #[test]
    fn icon_digest_matches_manifest() {
        let manifest: Value = serde_json::from_str(MANIFEST).expect("manifest parses");

        assert_eq!(manifest["assets"][0]["sha256"], ICON_SHA256);
        assert_eq!(format!("{:x}", Sha256::digest(ICON)), ICON_SHA256);
    }

    #[test]
    fn language_mapping_matches_upstream_variants() {
        assert_eq!(
            LanguageVariant::from_source_code("es-419").api_code,
            "es-la"
        );
        assert_eq!(LanguageVariant::from_source_code("pt-BR").api_code, "pt-br");
        assert_eq!(
            LanguageVariant::from_api_code("es-la").source_code,
            "es-419"
        );
        assert_eq!(LanguageVariant::from_api_code("pt-br").source_code, "pt-BR");
        assert_eq!(LanguageVariant::from_source_code("pt").api_code, "pt");
    }

    #[test]
    fn serializes_search_filters_and_author_preflight() {
        let filters = json!({
            "types": { "manga": true, "manhwa": true, "other": false },
            "genres_mode": "and",
            "genres": { "1": true, "5": false, "39": null },
            "statuses": { "finished": true, "releasing": false },
            "author": "Kishimoto",
            "year_from": "2010",
            "year_to": "2024",
            "min_chap": "10",
            "sort": "views_total:desc"
        });

        let tag_url = tag_lookup_url("Kishimoto").expect("tag url");
        assert_eq!(
            Url::parse(&tag_url)
                .expect("url")
                .query_pairs()
                .collect::<Vec<_>>(),
            vec![("keyword".into(), "Kishimoto".into())]
        );

        let url = search_url("Naruto", 2, &filters, Some("150932")).expect("search url");
        let query = Url::parse(&url)
            .expect("url")
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();

        assert!(query.contains(&("keyword".into(), "Naruto".into())));
        assert!(query.contains(&("page".into(), "2".into())));
        assert!(query.contains(&("limit".into(), "50".into())));
        assert!(query.contains(&("authors[]".into(), "150932".into())));
        assert!(query.contains(&("types[]".into(), "manga".into())));
        assert!(query.contains(&("types[]".into(), "manhwa".into())));
        assert!(query.contains(&("genres_mode".into(), "and".into())));
        assert!(query.contains(&("genres_in[]".into(), "1".into())));
        assert!(query.contains(&("genres_ex[]".into(), "5".into())));
        for blocked in PLAY_BLOCKED_GENRE_IDS {
            assert!(query.contains(&("genres_ex[]".into(), blocked.into())));
        }
        for rating in PLAY_ALLOWED_CONTENT_RATINGS {
            assert!(query.contains(&("content_rating[]".into(), rating.into())));
        }
        assert!(query.contains(&("statuses[]".into(), "finished".into())));
        assert!(query.contains(&("year_from".into(), "2010".into())));
        assert!(query.contains(&("year_to".into(), "2024".into())));
        assert!(query.contains(&("min_chap".into(), "10".into())));
        assert!(query.contains(&("order[views_total]".into(), "desc".into())));
    }

    #[test]
    fn uses_default_genre_mode_and_sort_when_filters_are_empty() {
        let url = search_url("Blue Lock", 1, &json!({}), None).expect("search url");
        let query = Url::parse(&url)
            .expect("url")
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();

        assert!(query.contains(&("genres_mode".into(), "and".into())));
        assert!(query.contains(&("order[relevance]".into(), "desc".into())));
        for blocked in PLAY_BLOCKED_GENRE_IDS {
            assert!(query.contains(&("genres_ex[]".into(), blocked.into())));
        }
        for rating in PLAY_ALLOWED_CONTENT_RATINGS {
            assert!(query.contains(&("content_rating[]".into(), rating.into())));
        }
    }

    #[test]
    fn signs_api_requests_with_upstream_sorted_array_semantics() {
        let signed = signed_api_url(
            "https://mangafire.to/api/titles?content_rating%5B%5D=suggestive&page=1&content_rating%5B%5D=safe&limit=50",
        )
        .expect("signed API URL");
        let url = Url::parse(&signed).expect("signed URL parses");
        let query = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>();

        assert_eq!(
            query,
            vec![
                ("content_rating[]".to_owned(), "suggestive".to_owned()),
                ("content_rating[]".to_owned(), "safe".to_owned()),
                ("limit".to_owned(), "50".to_owned()),
                ("page".to_owned(), "1".to_owned()),
                (
                    "vrf".to_owned(),
                    "8sK3xtqdFZdOu6WNqS1bZ0shnUDqyRXMnh4NiR8jL8tWJ0vrNk9ygltAj7dHSy6V9oIm8rqHhDViwGuQlOsQX2U3Cxyyix5QTit9"
                        .to_owned(),
                ),
            ]
        );
    }

    #[test]
    fn maps_catalog_and_details_fixtures() {
        let popular: ApiResponse<MangaDto> =
            serde_json::from_str(POPULAR_FIXTURE).expect("popular fixture parses");
        let page = MangaFireSource::filtered_catalog_page(popular);
        assert_eq!(page.entries.len(), 2);
        assert_eq!(page.entries[0].key, "/title/kw9j9-blue-lockk");
        assert_eq!(page.entries[0].title, "Blue Lock");
        assert_eq!(
            page.entries[0]
                .cover
                .as_ref()
                .map(|request| request.url.as_str()),
            Some("https://static.mfcdn.nl/4b71/i/f/0c/poster.jpg")
        );
        assert!(page.has_next_page);

        let details: MangaDetailsResponse =
            serde_json::from_str(DETAILS_FIXTURE).expect("details fixture parses");
        let item = details.data.into_catalog_item().expect("details mapping");
        assert_eq!(item.key, "/title/kw9j9-blue-lockk");
        assert_eq!(item.authors, vec!["KANESHIRO Muneyuki"]);
        assert_eq!(item.artists, vec!["NOMURA Yusuke"]);
        assert_eq!(item.status, Some(json!("ongoing")));
        assert_eq!(
            item.description.as_deref(),
            Some("Japan chases the perfect striker. Victory demands ego.")
        );
        assert_eq!(item.tags, vec!["Manga", "Sports", "Drama", "School"]);
        assert_eq!(
            item.extra.get("aliases"),
            Some(&json!(["Buruu Rokku", "Blue Lock"]))
        );
    }

    #[test]
    fn rejects_unknown_and_blocked_content_before_details_are_exposed() {
        let mut safe: Value = serde_json::from_str(DETAILS_FIXTURE).expect("details fixture");
        safe["data"]["contentRating"] = Value::Null;
        let unknown: MangaDetailsResponse = serde_json::from_value(safe).expect("unknown details");
        assert!(unknown.data.ensure_play_allowed().is_err());

        let mut adult: Value = serde_json::from_str(DETAILS_FIXTURE).expect("details fixture");
        adult["data"]["contentRating"] = json!("pornographic");
        let adult: MangaDetailsResponse = serde_json::from_value(adult).expect("adult details");
        assert!(adult.data.ensure_play_allowed().is_err());

        let mut ecchi: Value = serde_json::from_str(DETAILS_FIXTURE).expect("details fixture");
        ecchi["data"]["genres"] = json!([{ "title": "Ecchi" }]);
        let ecchi: MangaDetailsResponse = serde_json::from_value(ecchi).expect("ecchi details");
        assert!(ecchi.data.ensure_play_allowed().is_err());
    }

    #[test]
    fn selects_author_tag_before_artist() {
        let tags: TagResponse = serde_json::from_str(TAGS_FIXTURE).expect("tags fixture parses");
        assert_eq!(select_author_tag(&tags).as_deref(), Some("150932"));
    }

    #[test]
    fn maps_paginated_chapters_and_pages() {
        let page_one: ApiResponse<ChapterDto> =
            serde_json::from_str(CHAPTERS_PAGE_1_FIXTURE).expect("chapter page 1 parses");
        let page_two: ApiResponse<ChapterDto> =
            serde_json::from_str(CHAPTERS_PAGE_2_FIXTURE).expect("chapter page 2 parses");
        let language = LanguageVariant::from_source_code("en");

        let mut chapters = Vec::new();
        for (index, chapter) in page_one.items.into_iter().chain(page_two.items).enumerate() {
            chapters.push(
                chapter
                    .into_manga_chapter("/title/kw9j9-blue-lockk", language, index as i32)
                    .expect("chapter maps"),
            );
        }

        assert_eq!(chapters.len(), 3);
        assert_eq!(
            chapters[0].key,
            "/title/kw9j9-blue-lockk/8981099-chapter-353-en"
        );
        assert_eq!(chapters[0].chapter_number, Some(353.0));
        assert_eq!(chapters[0].language.as_deref(), Some("en"));
        assert_eq!(
            chapters[2].key,
            "/title/kw9j9-blue-lockk/7668016-chapter-352-es-la"
        );
        assert_eq!(chapters[2].language.as_deref(), Some("es-419"));
        assert_eq!(
            chapters[2].title.as_deref(),
            Some("Ch. 352 - The Puppet Nobody Wants")
        );

        let pages: PagesResponse =
            serde_json::from_str(PAGES_FIXTURE).expect("pages fixture parses");
        let mapped = pages
            .data
            .pages
            .into_iter()
            .enumerate()
            .map(|(index, page)| page.into_manga_page(index).expect("page maps"))
            .collect::<Vec<_>>();
        assert_eq!(mapped.len(), 2);
        match &mapped[0].content {
            PageContent::Url { url, context } => {
                assert_eq!(url, "https://nw8.mfcdn1.xyz/mf/abc/h/p.jpg");
                let context = context.as_ref().expect("page request context");
                assert_eq!(
                    context.get("User-Agent").map(String::as_str),
                    Some(BROWSER_USER_AGENT)
                );
                assert_eq!(context.get("Referer").map(String::as_str), Some(REFERER));
            }
            other => panic!("expected direct page url, got {other:?}"),
        }
    }

    #[test]
    fn deep_link_parsing_covers_item_and_chapter_urls() {
        assert_eq!(
            item_path_from_candidate("/title/kw9j9-blue-lockk/8981099-chapter-353-en")
                .expect("item path"),
            "/title/kw9j9-blue-lockk"
        );
        assert_eq!(
            chapter_id_from_path("/title/kw9j9-blue-lockk/8981099-chapter-353-en")
                .expect("chapter id"),
            8981099
        );
        assert_eq!(
            chapter_number_from_path("/title/kw9j9-blue-lockk/8981099-chapter-353-en"),
            Some(353.0)
        );
        assert_eq!(
            chapter_api_language_from_path("/title/kw9j9-blue-lockk/8981099-chapter-353-en"),
            Some("en".to_owned())
        );
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        assert!(serde_json::from_str::<ApiResponse<MangaDto>>(
            r#"{"items":[{"hid":123,"title":"Broken"}]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<MangaDetailsResponse>(
            r#"{"data":{"hid":"kw9j9","title":42}}"#
        )
        .is_err());
        assert!(
            serde_json::from_str::<PagesResponse>(r#"{"data":{"pages":[{"url":1}]}}"#).is_err()
        );
        assert!(chapter_id_from_path("/title/kw9j9-blue-lockk/not-a-chapter").is_err());
    }
}
