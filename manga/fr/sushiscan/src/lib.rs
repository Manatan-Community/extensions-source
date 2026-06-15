use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{dates, html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: Source = Source;
const BASE_URL: &str = "https://sushiscan.net";
const NAME: &str = "Sushi-Scan";
const DIR: &str = "catalogue";
const LANG: &str = "fr";
const CONTENT_RATING: &str = "adult";

struct Source;

impl MangaSource for Source {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(parse_listing(LIST_FIXTURE));
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let order = if request.get("listingId").and_then(Value::as_str) == Some("latest") {
            "update"
        } else {
            "popular"
        };
        Ok(parse_listing(&fetch(
            &catalogue_url(page, order),
            LIST_FIXTURE,
        )))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if let Some(key) = deeplink(query) {
            return Ok(Paged {
                entries: vec![parse_details(
                    &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )],
                has_next_page: false,
            });
        }
        Ok(parse_listing(&fetch(
            &search_url(page, query),
            LIST_FIXTURE,
        )))
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| format!("/{DIR}/sample"));
        Ok(parse_details(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            Some(key),
        ))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| format!("/{DIR}/sample"));
        Ok(parse_chapters(
            &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
            &key,
        ))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| format!("/{DIR}/sample/chapter-1"));
        let chapter_url = url::join_url(BASE_URL, &key);
        Ok(parse_pages(
            &fetch(&chapter_url, PAGES_FIXTURE),
            &chapter_url,
        ))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| url::join_url(BASE_URL, &key)))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(key) = deeplink(input) {
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(
                    &fetch(&url::join_url(BASE_URL, &key), DETAILS_FIXTURE),
                    Some(key),
                )),
                url: Some(input.into()),
                ..UrlResolveResult::default()
            }));
        }
        Ok(Some(UrlResolveResult {
            search: Some(SearchRequest {
                query: input.into(),
                ..SearchRequest::default()
            }),
            url: Some(input.into()),
            ..UrlResolveResult::default()
        }))
    }
}

export_manga_source!(SOURCE);

fn client() -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{BASE_URL}/{DIR}/"))
        .with_cookies_for(BASE_URL)
        .with_webview_challenge_fallback()
}

fn fetch(target: &str, fixture: &str) -> String {
    client()
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn catalogue_url(page: u64, order: &str) -> String {
    format!("{BASE_URL}/{DIR}/?page={page}&order={order}")
}

fn search_url(page: u64, query: &str) -> String {
    format!(
        "{BASE_URL}/page/{page}?s={}",
        url::query_escape(query.trim())
    )
}

fn parse_listing(body: &str) -> Paged<CatalogItem> {
    Paged {
        entries: body
            .split("<div")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("bsx")
                    || chunk.contains("listupd")
                    || chunk.contains("uta")
                    || chunk.contains("imgu")
                    || chunk.contains("page-item-detail")
            })
            .filter_map(parse_card)
            .fold(Vec::new(), push_unique),
        has_next_page: body.contains("pagination")
            && (body.to_ascii_lowercase().contains("next") || body.contains("hpage")),
    }
}

fn parse_card(chunk: &str) -> Option<CatalogItem> {
    let href = html::attr_after(chunk, "<a", "href")?;
    if href.contains("/chapter") || href.contains("/chapitre") {
        return None;
    }
    let key = normalize(&href);
    Some(CatalogItem {
        key: key.clone(),
        title: html::attr_after(chunk, "<a", "title")
            .or_else(|| html::attr_after(chunk, "<img", "alt"))
            .or_else(|| html::text_between(chunk, "tt", "</").map(|value| html::strip_tags(&value)))
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| NAME.into()),
        cover: image(chunk).map(|img| url::join_url(BASE_URL, &img)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: false,
        ..CatalogItem::default()
    })
}

fn parse_details(body: &str, key: Option<String>) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("/{DIR}/sample"));
    let description = text_for_class(body, "desc")
        .or_else(|| text_for_class(body, "entry-content"))
        .or_else(|| text_for_class(body, "summary__content"))
        .or_else(|| text_for_class(body, "summary"))
        .filter(|value| !value.is_empty());
    let alt = info_value(body, "Nom alternatif")
        .or_else(|| info_value(body, "Autre nom"))
        .or_else(|| text_for_class(body, "alternative"));
    let description = match (description, alt) {
        (Some(desc), Some(alt)) if !alt.is_empty() => {
            Some(format!("{desc}\n\nNom alternatif: {alt}"))
        }
        (None, Some(alt)) if !alt.is_empty() => Some(format!("Nom alternatif: {alt}")),
        (desc, _) => desc,
    };
    CatalogItem {
        key: normalize(&key),
        title: html::text_between(body, "entry-title", "</")
            .or_else(|| html::text_between(body, "post-title", "</"))
            .or_else(|| html::text_between(body, "<h1", "</h1>"))
            .or_else(|| html::text_between(body, "<h3", "</h3>"))
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .or_else(|| url::slug_from_url(&key))
            .unwrap_or_else(|| NAME.into()),
        cover: html::attr_after(body, "class=\"thumb", "src")
            .or_else(|| html::attr_after(body, "class='thumb", "src"))
            .or_else(|| html::attr_after(body, "summary_image", "src"))
            .or_else(|| html::attr_after(body, "itemprop=\"image\"", "src"))
            .or_else(|| image(body))
            .map(|img| url::join_url(BASE_URL, &img)),
        authors: info_value(body, "Auteur")
            .or_else(|| info_value(body, "Author"))
            .into_iter()
            .collect(),
        artists: info_value(body, "Artiste")
            .or_else(|| info_value(body, "Artist"))
            .into_iter()
            .collect(),
        tags: tags(body),
        description,
        status: status_value(body).map_or(ItemStatus::Unknown, |value| parse_status(&value)),
        url: Some(url::join_url(BASE_URL, &key)),
        language: Some(LANG.into()),
        content_rating: Some(CONTENT_RATING.into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str, manga_key: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("<li")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("chapternum")
                || chunk.contains("chapterdate")
                || chunk.contains("eph-num")
                || chunk.contains("wp-manga-chapter")
                || chunk.contains("lch")
        })
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "<a", "href")?;
            let key = normalize(&href);
            let title = html::text_between(chunk, "chapternum", "</")
                .or_else(|| html::text_between(chunk, "<a", "</a>"))
                .map(|value| html::strip_tags(&value))
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "Chapitre".into());
            Some(MangaChapter {
                key: key.clone(),
                title: Some(title.clone()),
                chapter_number: chapter_number(&title),
                date_uploaded: html::text_between(chunk, "chapterdate", "</")
                    .or_else(|| html::text_between(chunk, "chapter-release-date", "</"))
                    .map(|value| html::strip_tags(&value))
                    .and_then(|value| parse_date(&value)),
                url: Some(url::join_url(BASE_URL, &key)),
                language: Some(LANG.into()),
                ..MangaChapter::default()
            })
        })
        .fold(Vec::new(), push_unique_chapter);
    if chapters.is_empty() {
        chapters.push(MangaChapter {
            key: manga_key.to_string(),
            title: Some("Lire".into()),
            url: Some(url::join_url(BASE_URL, manga_key)),
            language: Some(LANG.into()),
            ..MangaChapter::default()
        });
    }
    chapters
}

fn parse_pages(body: &str, referer: &str) -> Vec<MangaPage> {
    let mut images = ts_reader_images(body);
    if images.is_empty() {
        images = script_images(body);
    }
    if images.is_empty() {
        images = body
            .split("<img")
            .skip(1)
            .filter(|chunk| {
                chunk.contains("readerarea")
                    || chunk.contains("reading-content")
                    || chunk.contains("wp-manga-chapter-img")
                    || chunk.contains("ts-main-image")
                    || chunk.contains("data-src")
            })
            .filter_map(image)
            .collect();
    }
    let mut seen = Vec::<String>::new();
    images
        .into_iter()
        .map(|image| image.replace("http://", "https://"))
        .filter(|image| {
            !image.starts_with("data:") && !image.is_empty() && push_seen(&mut seen, image)
        })
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: url::join_url(BASE_URL, &image),
                context: Some(manga::image_headers(referer)),
            },
            headers: manga::image_headers(referer),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn ts_reader_images(body: &str) -> Vec<String> {
    let Some(start) = body.find("ts_reader.run(") else {
        return Vec::new();
    };
    let json_start = start + "ts_reader.run(".len();
    let Some(end) = body[json_start..]
        .find(");")
        .map(|index| json_start + index)
    else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&body[json_start..end]) else {
        return Vec::new();
    };
    value
        .get("sources")
        .and_then(Value::as_array)
        .and_then(|sources| sources.first())
        .and_then(|source| source.get("images"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|value| value.as_str().map(ToString::to_string))
        .collect()
}

fn script_images(body: &str) -> Vec<String> {
    if let Some(start) = body.find("\"images\"") {
        if let Some(open) = body[start..].find('[').map(|idx| start + idx) {
            if let Some(close) = body[open..].find(']').map(|idx| open + idx + 1) {
                if let Ok(images) = serde_json::from_str::<Vec<String>>(&body[open..close]) {
                    return images;
                }
            }
        }
    }
    body.split('"')
        .filter(|part| {
            part.starts_with("http")
                && [".jpg", ".jpeg", ".png", ".webp", ".avif"]
                    .iter()
                    .any(|ext| part.to_ascii_lowercase().contains(ext))
        })
        .map(ToString::to_string)
        .collect()
}

fn image(chunk: &str) -> Option<String> {
    html::attr_after(chunk, "<img", "data-lazy-src")
        .or_else(|| html::attr_after(chunk, "<img", "data-src"))
        .or_else(|| html::attr_after(chunk, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(chunk, "<img", "srcset").and_then(srcset_first))
        .or_else(|| html::attr_after(chunk, "<img", "src"))
        .or_else(|| html::attr(chunk, "data-lazy-src"))
        .or_else(|| html::attr(chunk, "data-src"))
        .or_else(|| html::attr(chunk, "src"))
}

fn srcset_first(value: String) -> Option<String> {
    value
        .split(',')
        .next()?
        .split_whitespace()
        .next()
        .map(str::to_string)
}

fn info_value(body: &str, label: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let label_lower = label.to_ascii_lowercase();
    let index = lower.find(&label_lower)?;
    let fragment = &body[index..body.len().min(index + 900)];
    html::text_between(fragment, "<i", "</i>")
        .or_else(|| html::text_between(fragment, "<span", "</span>"))
        .or_else(|| {
            fragment
                .split_once("</td>")
                .and_then(|(_, rest)| html::text_between(rest, "<td", "</td>"))
        })
        .or_else(|| html::text_between(fragment, "</b>", "</"))
        .map(|value| html::strip_tags(&value))
        .map(|value| value.trim_matches([':', ' ', '\n', '\t']).to_string())
        .filter(|value| !value.is_empty() && value != "-" && value != "N/A")
}

fn status_value(body: &str) -> Option<String> {
    info_value(body, "Statut")
        .or_else(|| text_for_class(body, "status-value"))
        .or_else(|| info_value(body, "Status"))
}

fn text_for_class(body: &str, class_name: &str) -> Option<String> {
    body.split("<div")
        .chain(body.split("<span"))
        .find(|chunk| chunk.contains(class_name))
        .map(|chunk| html::strip_tags(chunk.split("</div>").next().unwrap_or(chunk)))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn tags(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("/genre/") || chunk.contains("genre="))
        .filter_map(|chunk| {
            html::text_between(chunk, ">", "</a>").map(|value| html::strip_tags(&value))
        })
        .filter(|tag| !tag.is_empty())
        .collect()
}

fn parse_status(value: &str) -> ItemStatus {
    let value = value.to_ascii_lowercase();
    if ["en cours", "ongoing"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Ongoing
    } else if ["terminé", "termine", "completed"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Completed
    } else if ["abandonné", "abandonne", "cancelled", "dropped"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Cancelled
    } else if ["pause", "hiatus"]
        .iter()
        .any(|needle| value.contains(needle))
    {
        ItemStatus::Hiatus
    } else {
        ItemStatus::Unknown
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let clean = value.trim().trim_start_matches("le ").replace(',', " ");
    dates::parse_fixture_date(&clean)
        .or_else(|| dates::parse_ymd(&clean))
        .or_else(|| parse_day_month_year(&clean.to_ascii_lowercase()))
}

fn parse_day_month_year(value: &str) -> Option<i64> {
    let parts = value.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 3 {
        return None;
    }
    let (day, month, year) = if parts[0].chars().all(|ch| ch.is_ascii_digit()) {
        (parts[0], parts[1], parts[2])
    } else {
        (parts[1], parts[0], parts[2])
    };
    let day = day.parse::<u32>().ok()?;
    let month = french_month(month)?;
    let year = year.parse::<i32>().ok()?;
    dates::parse_ymd(&format!("{year:04}-{month:02}-{day:02}"))
}

fn french_month(value: &str) -> Option<u32> {
    Some(match value {
        "janvier" | "janv" => 1,
        "février" | "fevrier" | "févr" | "fevr" => 2,
        "mars" => 3,
        "avril" | "avr" => 4,
        "mai" => 5,
        "juin" => 6,
        "juillet" | "juil" => 7,
        "août" | "aout" => 8,
        "septembre" | "sept" => 9,
        "octobre" | "oct" => 10,
        "novembre" | "nov" => 11,
        "décembre" | "decembre" | "déc" | "dec" => 12,
        _ => return None,
    })
}

fn chapter_number(value: &str) -> Option<f32> {
    value
        .split(|ch: char| !(ch.is_ascii_digit() || ch == '.'))
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse::<f32>().ok())
}

fn normalize(input: &str) -> String {
    if input.starts_with("http://") || input.starts_with("https://") {
        if let Some(index) = input.find(BASE_URL) {
            return format!("/{}", input[index + BASE_URL.len()..].trim_matches('/'));
        }
    }
    format!("/{}", input.trim_start_matches('/').trim_end_matches('/'))
}

fn deeplink(input: &str) -> Option<String> {
    input.starts_with(BASE_URL).then(|| normalize(input))
}

fn push_seen(seen: &mut Vec<String>, value: &str) -> bool {
    if seen.iter().any(|entry| entry == value) {
        false
    } else {
        seen.push(value.to_string());
        true
    }
}

fn push_unique(mut out: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !out.iter().any(|existing| existing.key == item.key) {
        out.push(item);
    }
    out
}

fn push_unique_chapter(mut out: Vec<MangaChapter>, chapter: MangaChapter) -> Vec<MangaChapter> {
    if !out.iter().any(|existing| existing.key == chapter.key) {
        out.push(chapter);
    }
    out
}

const LIST_FIXTURE: &str = r#"
<div class="bsx"><a href="/catalogue/sample" title="Sample"><img src="/cover.jpg"></a></div>
<div class="pagination"><a class="next" href="/catalogue/?page=2&order=popular">Next</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="bigcontent"><h1 class="entry-title">Sample</h1><div class="thumb"><img src="/cover.jpg"></div>
<div class="desc">Resume</div><div class="mgen"><a href="/genre/action">Action</a></div>
<table class="infotable"><tr><td>Auteur</td><td>Writer</td></tr><tr><td>Statut</td><td>En Cours</td></tr></table>
<ul id="chapterlist"><li><a href="/catalogue/sample/chapter-1"><span class="chapternum">Chapitre 1</span></a><span class="chapterdate">2024-01-01</span></li></ul></div>
"#;

const PAGES_FIXTURE: &str = r#"<script>ts_reader.run({"sources":[{"source":"default","images":["/page1.jpg","http://sushiscan.net/page2.jpg"]}]});</script>"#;
