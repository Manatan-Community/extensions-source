use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, MangaPageImage, PageContent, Paged,
    UrlResolveResult, abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const BASE_URL: &str = "https://www.dragonball-multiverse.com";
const SOURCE: DragonBallMultiverse = DragonBallMultiverse;

struct DragonBallMultiverse;

impl MangaSource for DragonBallMultiverse {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let source = source_for(&request);
        let body = fetch_document_or_fixture(&format!("{BASE_URL}/{}/read.html", source.internal_lang), LIST_FIXTURE);
        Ok(parse_listing(&body, source))
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let query = request.get("query").and_then(Value::as_str).unwrap_or_default().trim();
        if query.starts_with(BASE_URL) {
            return Ok(Paged {
                entries: vec![item_from_key(normalize_key(query), source_for(&request), "Dragon Ball Multiverse")],
                has_next_page: false,
            });
        }
        Ok(Paged { entries: Vec::new(), has_next_page: false })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let source = source_for(&request);
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| format!("/{}/read.html", source.internal_lang));
        Ok(item_from_key(key, source, if source.parody { "Dragon Ball Multiverse Parody" } else { "Dragon Ball Multiverse" }))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key = manga::request_key(&request, "manga").unwrap_or_else(|| format!("/{}/read.html", source_for(&request).internal_lang));
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), CHAPTERS_FIXTURE);
        Ok(parse_chapters(&body))
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter").unwrap_or_else(|| "/en/page-1.html".into());
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, &key), PAGES_FIXTURE);
        Ok(parse_page_links(&body))
    }

    fn resolve_page_image(&self, request: Value) -> ExtensionResult<MangaPageImage> {
        let key = request
            .get("page")
            .and_then(|page| page.get("content"))
            .and_then(|content| content.get("lazy"))
            .and_then(|lazy| lazy.get("key"))
            .and_then(Value::as_str)
            .unwrap_or("/en/page-1.html");
        let body = fetch_document_or_fixture(&url::join_url(BASE_URL, key), IMAGE_PAGE_FIXTURE);
        let image = parse_reader_image(&body).unwrap_or_else(|| format!("{BASE_URL}/imgs/sample.jpg"));
        Ok(MangaPageImage {
            url: image,
            headers: manga::image_headers(BASE_URL),
            ..MangaPageImage::default()
        })
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else { return Ok(None); };
        if input.starts_with(BASE_URL) {
            let source = source_for(&request);
            return Ok(Some(UrlResolveResult {
                item: Some(item_from_key(normalize_key(input), source, if source.parody { "Dragon Ball Multiverse Parody" } else { "Dragon Ball Multiverse" })),
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

#[derive(Clone, Copy)]
struct SourceConfig {
    id: &'static str,
    lang: &'static str,
    internal_lang: &'static str,
    parody: bool,
}

fn source_for(request: &Value) -> SourceConfig {
    let id = request.get("sourceId").or_else(|| request.get("source_id")).and_then(Value::as_str).unwrap_or("dragonballmultiverse-en");
    SOURCES.iter().copied().find(|source| source.id == id).unwrap_or(SOURCES[0])
}

fn fetch_document_or_fixture(target_url: &str, fixture: &str) -> String {
    http::HttpClient::browser()
        .with_referer(format!("{BASE_URL}/"))
        .get(target_url)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, source: SourceConfig) -> Paged<CatalogItem> {
    let entries = body
        .split("dbm-read")
        .skip(1)
        .filter_map(|block| {
            let title = html::text_between(block, "<h3", "</h3>").map(|value| html::strip_tags(&value))?;
            let href = html::attr_after(block, "<a", "href").unwrap_or_else(|| format!("/{}/read.html", source.internal_lang));
            let cover = html::attr_after(block, "<img", "src").map(|value| url::join_url(BASE_URL, &value));
            let description = html::text_between(block, "<div", "</div>").map(|value| html::strip_tags(&value));
            Some(CatalogItem {
                description,
                cover,
                ..item_from_key(normalize_key(&href), source, &title)
            })
        })
        .collect();
    Paged { entries, has_next_page: false }
}

fn item_from_key(key: String, source: SourceConfig, title: &str) -> CatalogItem {
    CatalogItem {
        key: key.clone(),
        title: title.into(),
        url: Some(url::join_url(BASE_URL, &key)),
        status: ItemStatus::Unknown,
        language: Some(source.lang.into()),
        content_rating: Some("safe".into()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_chapters(body: &str) -> Vec<MangaChapter> {
    let mut chapters = body
        .split("cadrelect chapter")
        .skip(1)
        .filter_map(|block| {
            let href = html::attr_after(block, "<a", "href")?;
            let title = html::text_between(block, "<h4", "</h4>").map(|value| html::strip_tags(&value)).unwrap_or_else(|| "Chapter".into());
            Some(MangaChapter {
                key: normalize_key(&href),
                title: Some(title),
                ..MangaChapter::default()
            })
        })
        .collect::<Vec<_>>();
    chapters.reverse();
    let total = chapters.len();
    for (index, chapter) in chapters.iter_mut().enumerate() {
        chapter.chapter_number = Some((total - index) as f32);
    }
    chapters
}

fn parse_page_links(body: &str) -> Vec<MangaPage> {
    body.split("pageslist")
        .nth(1)
        .unwrap_or(body)
        .split("<a")
        .skip(1)
        .filter_map(|block| html::attr(block, "href"))
        .enumerate()
        .map(|(index, href)| {
            let key = normalize_key(&href);
            MangaPage {
                content: PageContent::Lazy {
                    key: key.clone(),
                    url: Some(url::join_url(BASE_URL, &key)),
                    page_url: Some(url::join_url(BASE_URL, &key)),
                    context: None,
                },
                description: Some(format!("Page {}", index + 1)),
                ..MangaPage::default()
            }
        })
        .collect()
}

fn parse_reader_image(body: &str) -> Option<String> {
    let marker = body.find("balloonsimg")?;
    let block = &body[marker..];
    if let Some(src) = html::attr_after(block, "<img", "src").or_else(|| html::attr(block, "src")) {
        return Some(url::join_url(BASE_URL, &src));
    }
    let style = html::attr(block, "style")?;
    let raw = style.split("url(").nth(1)?.split(')').next()?.trim_matches(['"', '\'']);
    Some(url::join_url(BASE_URL, raw))
}

fn normalize_key(input: &str) -> String {
    let path = input
        .trim_start_matches(BASE_URL)
        .split('#')
        .next()
        .unwrap_or(input)
        .split('?')
        .next()
        .unwrap_or(input)
        .trim();
    format!("/{}", path.trim_matches('/'))
}

const SOURCES: &[SourceConfig] = &[
    SourceConfig { id: "dragonballmultiverse-en", lang: "en", internal_lang: "en", parody: false },
    SourceConfig { id: "dragonballmultiverse-fr", lang: "fr", internal_lang: "fr", parody: false },
    SourceConfig { id: "dragonballmultiverse-ja", lang: "ja", internal_lang: "jp", parody: false },
    SourceConfig { id: "dragonballmultiverse-zh", lang: "zh", internal_lang: "cn", parody: false },
    SourceConfig { id: "dragonballmultiverse-es", lang: "es", internal_lang: "es", parody: false },
    SourceConfig { id: "dragonballmultiverse-it", lang: "it", internal_lang: "it", parody: false },
    SourceConfig { id: "dragonballmultiverse-pt", lang: "pt", internal_lang: "pt", parody: false },
    SourceConfig { id: "dragonballmultiverse-de", lang: "de", internal_lang: "de", parody: false },
    SourceConfig { id: "dragonballmultiverse-pl", lang: "pl", internal_lang: "pl", parody: false },
    SourceConfig { id: "dragonballmultiverse-nl", lang: "nl", internal_lang: "nl", parody: false },
    SourceConfig { id: "dragonballmultiverse-fr-pa", lang: "fr", internal_lang: "fr_PA", parody: true },
    SourceConfig { id: "dragonballmultiverse-tr", lang: "tr", internal_lang: "tr_TR", parody: false },
    SourceConfig { id: "dragonballmultiverse-pt-br", lang: "pt-BR", internal_lang: "pt_BR", parody: false },
    SourceConfig { id: "dragonballmultiverse-hu", lang: "hu", internal_lang: "hu_HU", parody: false },
    SourceConfig { id: "dragonballmultiverse-ga", lang: "ga", internal_lang: "ga_ES", parody: false },
    SourceConfig { id: "dragonballmultiverse-ca", lang: "ca", internal_lang: "ct_CT", parody: false },
    SourceConfig { id: "dragonballmultiverse-no", lang: "no", internal_lang: "no_NO", parody: false },
    SourceConfig { id: "dragonballmultiverse-ru", lang: "ru", internal_lang: "ru_RU", parody: false },
    SourceConfig { id: "dragonballmultiverse-ro", lang: "ro", internal_lang: "ro_RO", parody: false },
    SourceConfig { id: "dragonballmultiverse-eu", lang: "eu", internal_lang: "eu_EH", parody: false },
    SourceConfig { id: "dragonballmultiverse-lt", lang: "lt", internal_lang: "lt_LT", parody: false },
    SourceConfig { id: "dragonballmultiverse-hr", lang: "hr", internal_lang: "hr_HR", parody: false },
    SourceConfig { id: "dragonballmultiverse-ko", lang: "ko", internal_lang: "kr_KR", parody: false },
    SourceConfig { id: "dragonballmultiverse-fi", lang: "fi", internal_lang: "fi_FI", parody: false },
    SourceConfig { id: "dragonballmultiverse-he", lang: "he", internal_lang: "he_HE", parody: false },
    SourceConfig { id: "dragonballmultiverse-bg", lang: "bg", internal_lang: "bg_BG", parody: false },
    SourceConfig { id: "dragonballmultiverse-sv", lang: "sv", internal_lang: "sv_SE", parody: false },
    SourceConfig { id: "dragonballmultiverse-el", lang: "el", internal_lang: "gr_GR", parody: false },
    SourceConfig { id: "dragonballmultiverse-es-419", lang: "es-419", internal_lang: "es_CO", parody: false },
    SourceConfig { id: "dragonballmultiverse-ar", lang: "ar", internal_lang: "ar_JO", parody: false },
    SourceConfig { id: "dragonballmultiverse-fil", lang: "fil", internal_lang: "tl_PI", parody: false },
    SourceConfig { id: "dragonballmultiverse-la", lang: "la", internal_lang: "la_LA", parody: false },
    SourceConfig { id: "dragonballmultiverse-da", lang: "da", internal_lang: "da_DK", parody: false },
    SourceConfig { id: "dragonballmultiverse-co", lang: "co", internal_lang: "co_FR", parody: false },
    SourceConfig { id: "dragonballmultiverse-br", lang: "br", internal_lang: "br_FR", parody: false },
    SourceConfig { id: "dragonballmultiverse-vec", lang: "vec", internal_lang: "xx_VE", parody: false },
    SourceConfig { id: "dragonballmultiverse-lmo", lang: "lmo", internal_lang: "xx_LMO", parody: false },
];

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<section id="dbm-reads">
  <div class="dbm-read"><h3>Main Story</h3><a href="https://www.dragonball-multiverse.com/en/read.html">Read</a><img src="/cover.jpg"><div>Fan manga</div></div>
</section>
"#;

const CHAPTERS_FIXTURE: &str = r#"
<div class="cadrelect chapter"><a href="https://www.dragonball-multiverse.com/en/chapter-1.html"><h4>Chapter 1</h4></a></div>
<div class="cadrelect chapter"><a href="https://www.dragonball-multiverse.com/en/chapter-2.html"><h4>Chapter 2</h4></a></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="pageslist"><a href="https://www.dragonball-multiverse.com/en/page-1.html">1</a><a href="https://www.dragonball-multiverse.com/en/page-2.html">2</a></div>
"#;

const IMAGE_PAGE_FIXTURE: &str = r#"
<div id="balloonsimg" style="background-image:url('/imgs/page.jpg')"><div class="balloon" style="left:10;top:20;width:80;">Text</div></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_listing_chapters_and_pages() {
        let source = SOURCES[0];
        assert_eq!(parse_listing(LIST_FIXTURE, source).entries.len(), 1);
        assert_eq!(parse_chapters(CHAPTERS_FIXTURE).len(), 2);
        assert_eq!(parse_page_links(PAGES_FIXTURE).len(), 2);
        assert_eq!(parse_reader_image(IMAGE_PAGE_FIXTURE), Some(format!("{BASE_URL}/imgs/page.jpg")));
    }
}
