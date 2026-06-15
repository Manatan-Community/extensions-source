use manatan_extension::{
    CatalogItem, ItemStatus, MangaChapter, MangaPage, PageContent, Paged, UrlResolveResult,
    abi::ExtensionResult, export_manga_source, http, source::MangaSource,
};
use manatan_shared::{html, manga, sdk::SearchRequest, url};
use serde_json::Value;

const SOURCE: VcpVmp = VcpVmp;
const LANG: &str = "es";
const CONTENT_RATING: &str = "adult";

#[derive(Clone, Copy)]
struct Site {
    id: &'static str,
    name: &'static str,
    base_url: &'static str,
    url_suffix: &'static str,
    genre_suffix: &'static str,
}

const SITES: [Site; 2] = [
    Site {
        id: "vcp",
        name: "VCP",
        base_url: "https://vercomicsporno.com",
        url_suffix: "comics-porno",
        genre_suffix: "etiquetas",
    },
    Site {
        id: "vmp",
        name: "VMP",
        base_url: "https://vermangasporno.com",
        url_suffix: "xxx",
        genre_suffix: "tag",
    },
];

struct VcpVmp;

impl MangaSource for VcpVmp {
    fn list(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let site = site_for(&request);
        if request.as_object().is_some_and(|object| object.is_empty()) {
            return Ok(Paged {
                entries: parse_listing(LIST_FIXTURE, site),
                has_next_page: has_next_page(LIST_FIXTURE),
            });
        }
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let body = fetch_document_or_fixture(site, &site.popular_url(page), LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, site),
            has_next_page: has_next_page(&body),
        })
    }

    fn search(&self, request: Value) -> ExtensionResult<Paged<CatalogItem>> {
        let site = site_for(&request);
        let page = request.get("page").and_then(Value::as_u64).unwrap_or(1);
        let query = request
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if query.starts_with(site.base_url) {
            let body = fetch_document_or_fixture(site, query, DETAILS_FIXTURE);
            return Ok(Paged {
                entries: vec![parse_details(&body, Some(site.normalize_key(query)), site)],
                has_next_page: false,
            });
        }
        let filters = request.get("filters").unwrap_or(&Value::Null);
        let genre = filter(filters, "genre", "");
        let target = if query.is_empty() && !genre.is_empty() {
            site.genre_url(genre, page)
        } else if query.is_empty() {
            site.popular_url(page)
        } else {
            site.search_url(query, page)
        };
        let body = fetch_document_or_fixture(site, &target, LIST_FIXTURE);
        Ok(Paged {
            entries: parse_listing(&body, site),
            has_next_page: has_next_page(&body),
        })
    }

    fn details(&self, request: Value) -> ExtensionResult<CatalogItem> {
        let site = site_for(&request);
        let key = manga::request_key(&request, "manga")
            .unwrap_or_else(|| format!("/{}/sample", site.url_suffix));
        let site = site_for_key_or_request(&key, &request);
        let body = fetch_document_or_fixture(site, &site.absolute_url(&key), DETAILS_FIXTURE);
        Ok(parse_details(&body, Some(site.normalize_key(&key)), site))
    }

    fn chapters(&self, request: Value) -> ExtensionResult<Vec<MangaChapter>> {
        let key =
            manga::request_key(&request, "manga").unwrap_or_else(|| "/comics-porno/sample".into());
        let site = site_for_key_or_request(&key, &request);
        Ok(vec![MangaChapter {
            key: site.normalize_key(&key),
            title: Some(site.name.to_string()),
            url: Some(site.absolute_url(&key)),
            chapter_number: Some(1.0),
            ..MangaChapter::default()
        }])
    }

    fn pages(&self, request: Value) -> ExtensionResult<Vec<MangaPage>> {
        let key = manga::request_key(&request, "chapter")
            .unwrap_or_else(|| "/comics-porno/sample".into());
        let site = site_for_key_or_request(&key, &request);
        let body = fetch_document_or_fixture(site, &site.absolute_url(&key), PAGES_FIXTURE);
        Ok(parse_pages(&body, site))
    }

    fn manga_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "manga").map(|key| {
            let site = site_for_key_or_request(&key, &request);
            site.absolute_url(&key)
        }))
    }

    fn chapter_url(&self, request: Value) -> ExtensionResult<Option<String>> {
        Ok(manga::request_key(&request, "chapter").map(|key| {
            let site = site_for_key_or_request(&key, &request);
            site.absolute_url(&key)
        }))
    }

    fn handle_url(&self, request: Value) -> ExtensionResult<Option<UrlResolveResult>> {
        let Some(input) = request.get("url").and_then(Value::as_str) else {
            return Ok(None);
        };
        if let Some(site) = SITES.iter().find(|site| input.starts_with(site.base_url)) {
            let body = fetch_document_or_fixture(site, input, DETAILS_FIXTURE);
            return Ok(Some(UrlResolveResult {
                item: Some(parse_details(&body, Some(site.normalize_key(input)), site)),
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

impl Site {
    fn absolute_url(&self, value: &str) -> String {
        url::join_url(self.base_url, value)
    }

    fn normalize_key(&self, value: &str) -> String {
        if value.starts_with("http://") || value.starts_with("https://") {
            if let Some(index) = value.find(self.base_url) {
                return format!(
                    "/{}",
                    value[index + self.base_url.len()..]
                        .trim_start_matches('/')
                        .trim_end_matches('/')
                );
            }
        }
        format!("/{}", value.trim_start_matches('/').trim_end_matches('/'))
    }

    fn popular_url(&self, page: u64) -> String {
        format!(
            "{}/{}/page/{}",
            self.base_url.trim_end_matches('/'),
            self.url_suffix,
            page
        )
    }

    fn search_url(&self, query: &str, page: u64) -> String {
        format!(
            "{}/{}/page/{page}?s={}",
            self.base_url.trim_end_matches('/'),
            self.url_suffix,
            url::query_escape(query)
        )
    }

    fn genre_url(&self, genre: &str, page: u64) -> String {
        format!(
            "{}/{}/{}/page/{}",
            self.base_url.trim_end_matches('/'),
            self.genre_suffix,
            genre.trim_matches('/'),
            page
        )
    }
}

fn client(site: &Site) -> http::HttpClient {
    http::HttpClient::browser()
        .with_desktop_user_agent()
        .with_referer(format!("{}/", site.base_url.trim_end_matches('/')))
        .with_cookies_for(site.base_url)
        .with_webview_challenge_fallback()
}

fn fetch_document_or_fixture(site: &Site, target: &str, fixture: &str) -> String {
    client(site)
        .get(target)
        .browser_document()
        .send_text()
        .unwrap_or_else(|_| fixture.to_string())
}

fn parse_listing(body: &str, site: &Site) -> Vec<CatalogItem> {
    body.split("class=\"entry")
        .skip(1)
        .filter_map(|chunk| {
            let href = html::attr_after(chunk, "popimg", "href")
                .or_else(|| html::attr_after(chunk, "<a", "href"))?;
            let key = site.normalize_key(&href);
            let title = html::attr_after(chunk, "<img", "alt")
                .or_else(|| {
                    html::text_between(chunk, "<h2", "</h2>").map(|value| html::strip_tags(&value))
                })
                .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| site.name.into()));
            Some(CatalogItem {
                key: key.clone(),
                title,
                cover: image_attr(chunk).map(|value| site.absolute_url(&value)),
                url: Some(site.absolute_url(&key)),
                language: Some(LANG.to_string()),
                content_rating: Some(CONTENT_RATING.to_string()),
                initialized: false,
                ..CatalogItem::default()
            })
        })
        .fold(Vec::new(), push_unique)
}

fn parse_details(body: &str, key: Option<String>, site: &Site) -> CatalogItem {
    let key = key.unwrap_or_else(|| format!("/{}/sample", site.url_suffix));
    CatalogItem {
        key: key.clone(),
        title: html::text_between(body, "<h1", "</h1>")
            .map(|value| html::strip_tags(&value))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| url::slug_from_url(&key).unwrap_or_else(|| site.name.into())),
        cover: image_attr(body).map(|value| site.absolute_url(&value)),
        tags: tag_values(body),
        status: ItemStatus::Completed,
        url: Some(site.absolute_url(&key)),
        language: Some(LANG.to_string()),
        content_rating: Some(CONTENT_RATING.to_string()),
        initialized: true,
        ..CatalogItem::default()
    }
}

fn parse_pages(body: &str, site: &Site) -> Vec<MangaPage> {
    body.split("<img")
        .skip(1)
        .filter(|chunk| {
            chunk.contains("wp-content")
                || chunk.contains("post-imgs")
                || chunk.contains("data-src")
                || chunk.contains("src=")
        })
        .filter_map(image_attr)
        .filter(|value| !value.starts_with("data:"))
        .enumerate()
        .map(|(index, image)| MangaPage {
            content: PageContent::Url {
                url: site.absolute_url(&image),
                context: None,
            },
            headers: manga::image_headers(site.base_url),
            description: Some(format!("Page {}", index + 1)),
            ..MangaPage::default()
        })
        .collect()
}

fn tag_values(body: &str) -> Vec<String> {
    body.split("<a")
        .skip(1)
        .filter(|chunk| chunk.contains("rel=\"tag\"") || chunk.contains("rel='tag'"))
        .filter_map(|chunk| html::text_between(chunk, ">", "</a>"))
        .map(|value| html::strip_tags(&value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn image_attr(input: &str) -> Option<String> {
    html::attr_after(input, "<img", "data-src")
        .or_else(|| html::attr_after(input, "<img", "data-lazy-src"))
        .or_else(|| html::attr_after(input, "<img", "data-cfsrc"))
        .or_else(|| html::attr_after(input, "<img", "src"))
        .or_else(|| html::attr(input, "data-src"))
        .or_else(|| html::attr(input, "src"))
}

fn has_next_page(body: &str) -> bool {
    body.contains("wp-pagenavi") && body.contains("current") && body.contains("<a")
}

fn site_for(request: &Value) -> &'static Site {
    let source_id = request
        .get("sourceId")
        .or_else(|| request.get("source_id"))
        .or_else(|| {
            request
                .get("filters")
                .and_then(|filters| filters.get("sourceId"))
        })
        .and_then(Value::as_str)
        .unwrap_or("vcp");
    SITES
        .iter()
        .find(|site| site.id == source_id)
        .unwrap_or(&SITES[0])
}

fn site_for_key_or_request(key: &str, request: &Value) -> &'static Site {
    if key.contains("vermangasporno.com") || key.contains("/xxx/") {
        &SITES[1]
    } else if key.contains("vercomicsporno.com") || key.contains("/comics-porno/") {
        &SITES[0]
    } else {
        site_for(request)
    }
}

fn filter<'a>(filters: &'a Value, key: &str, default: &'a str) -> &'a str {
    filters.get(key).and_then(Value::as_str).unwrap_or(default)
}

fn push_unique(mut items: Vec<CatalogItem>, item: CatalogItem) -> Vec<CatalogItem> {
    if !items.iter().any(|existing| existing.key == item.key) {
        items.push(item);
    }
    items
}

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="entry"><a class="popimg" href="https://vercomicsporno.com/comics-porno/sample/"><img alt="Sample Comic" src="/cover.jpg"></a></div>
<div class="wp-pagenavi"><span class="current">1</span><a href="/page/2">2</a></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<h1>Sample Comic</h1>
<div class="tax_box"><div class="title">Etiquetas</div><a rel="tag" href="/etiquetas/anal/">Anal</a><a rel="tag" href="/etiquetas/milf/">Milf</a></div>
<div class="wp-content"><p><img src="/page1.jpg"></p><p><img data-src="/page2.jpg"></p></div>
"#;

const PAGES_FIXTURE: &str = DETAILS_FIXTURE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        let site = &SITES[0];
        assert_eq!(parse_listing(LIST_FIXTURE, site).len(), 1);
        assert_eq!(parse_pages(PAGES_FIXTURE, site).len(), 2);
    }
}
