use manatan_sdk::{client::BROWSER_USER_AGENT, html, model::PageContent};
use natsuid_manga::{
    build_filter_definitions, build_search_form, chapter_list_url,
    extract_manga_id_from_detail_page, extract_nonce, genre_filter_url, nonce_url,
    parse_chapters_html, parse_genres_json, parse_listing_slugs, parse_pages_html,
    parse_rest_manga_details, parse_rest_manga_list, rest_manga_item_url, rest_manga_list_url,
    SearchFilters, SearchSort,
};
use serde_json::json;

#[test]
fn builds_nonce_and_search_urls() {
    assert_eq!(
        nonce_url("https://rawkuma.net").unwrap(),
        "https://rawkuma.net/wp-admin/admin-ajax.php?type=search_form&action=get_nonce"
    );
    assert_eq!(
        genre_filter_url("https://rawkuma.net").unwrap(),
        "https://rawkuma.net/wp-json/wp/v2/genre?per_page=100&page=1&orderby=count&order=desc"
    );
    assert_eq!(
        rest_manga_item_url("https://rawkuma.net", 101).unwrap(),
        "https://rawkuma.net/wp-json/wp/v2/manga/101?_embed"
    );
    assert_eq!(
        chapter_list_url("https://rawkuma.net", "101", 777).unwrap(),
        "https://rawkuma.net/wp-admin/admin-ajax.php?manga_id=101&page=777&action=chapter_list"
    );
}

#[test]
fn builds_exact_advanced_search_form_payload() {
    let filters = SearchFilters {
        genre_inclusion_mode: "AND".to_owned(),
        genre_exclusion_mode: "OR".to_owned(),
        included_genres: vec!["action".to_owned(), "drama".to_owned()],
        excluded_genres: vec!["adult".to_owned()],
        project_only: true,
        types: vec!["manga".to_owned(), "manhwa".to_owned()],
        statuses: vec!["ongoing".to_owned()],
        sort: SearchSort {
            value: "updated".to_owned(),
            ascending: true,
        },
    };

    assert_eq!(
        build_search_form("nonce-123", "  alpha  ", 2, &filters),
        vec![
            ("nonce".to_owned(), "nonce-123".to_owned()),
            ("inclusion".to_owned(), "AND".to_owned()),
            ("exclusion".to_owned(), "OR".to_owned()),
            ("page".to_owned(), "2".to_owned()),
            ("genre".to_owned(), "[\"action\",\"drama\"]".to_owned()),
            ("genre_exclude".to_owned(), "[\"adult\"]".to_owned()),
            ("author".to_owned(), "[]".to_owned()),
            ("artist".to_owned(), "[]".to_owned()),
            ("project".to_owned(), "1".to_owned()),
            ("type".to_owned(), "[\"manga\",\"manhwa\"]".to_owned()),
            ("status".to_owned(), "[\"ongoing\"]".to_owned()),
            ("order".to_owned(), "asc".to_owned()),
            ("orderby".to_owned(), "updated".to_owned()),
            ("query".to_owned(), "alpha".to_owned())
        ]
    );
}

#[test]
fn builds_rest_slug_lookup_url_in_listing_order() {
    let url = rest_manga_list_url(
        "https://rawkuma.net",
        &["alpha".to_owned(), "beta".to_owned()],
    )
    .unwrap();
    assert_eq!(
        url,
        "https://rawkuma.net/wp-json/wp/v2/manga?slug%5B%5D=alpha&slug%5B%5D=beta&per_page=3&_embed"
    );
}

#[test]
fn parses_nonce_listing_slugs_and_pagination() {
    let nonce = include_str!("fixtures/nonce.html");
    assert_eq!(extract_nonce(nonce).as_deref(), Some("nonce-123"));

    let document = html::document(include_str!("fixtures/listing.html"));
    let slugs = parse_listing_slugs(&document, "https://rawkuma.net").unwrap();
    assert_eq!(slugs, ["alpha", "beta"]);
}

#[test]
fn maps_rest_payloads_and_filters_novels() {
    let items = parse_rest_manga_list(
        include_str!("fixtures/rest-list.json"),
        "https://rawkuma.net",
        "ja",
        true,
    )
    .unwrap();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].title, "Alpha & Beta");
    assert_eq!(items[0].authors, ["Jane Doe"]);
    assert_eq!(items[0].artists, ["John Doe"]);
    assert_eq!(items[0].tags, ["Action", "Manga"]);
    assert_eq!(items[0].status, Some(json!("ongoing")));
    assert_eq!(
        items[0].cover.as_ref().map(|request| request.url.as_str()),
        Some("https://rawkuma.net/wp-content/uploads/alpha.jpg")
    );
    let cover = items[0].cover.as_ref().unwrap();
    assert_eq!(
        cover.headers.get("Referer").map(String::as_str),
        Some("https://rawkuma.net")
    );
    assert_eq!(
        cover.headers.get("User-Agent").map(String::as_str),
        Some(BROWSER_USER_AGENT)
    );
    assert_eq!(items[0].extra["mangaId"], json!(101));
    assert_eq!(items[0].extra["slug"], json!("alpha"));
}

#[test]
fn maps_rest_details_payload() {
    let item = parse_rest_manga_details(
        include_str!("fixtures/rest-details.json"),
        "https://rawkuma.net",
        "ja",
    )
    .unwrap();
    assert_eq!(item.title, "Alpha & Beta");
    assert_eq!(item.status, Some(json!("completed")));
    assert_eq!(item.description.as_deref(), Some("Detail body."));
}

#[test]
fn parses_dynamic_genre_filters() {
    let genres = parse_genres_json(include_str!("fixtures/genres.json")).unwrap();
    assert_eq!(genres.len(), 2);

    let filters = build_filter_definitions(&genres);
    let rendered = serde_json::to_value(&filters).unwrap();
    assert!(rendered.to_string().contains("genre_include"));
    assert!(rendered.to_string().contains("genre_exclude"));
    assert!(rendered.to_string().contains("Genre Inclusion Mode"));
}

#[test]
fn parses_chapters_and_pages() {
    let chapters = parse_chapters_html(
        &html::document(include_str!("fixtures/chapters.html")),
        "https://rawkuma.net/manga/alpha/",
        "div a",
        "span",
        "time",
        "datetime",
        "ja",
    )
    .unwrap();

    assert_eq!(chapters.len(), 2);
    assert_eq!(chapters[0].title.as_deref(), Some("Chapter 12"));
    assert_eq!(chapters[0].chapter_number, Some(12.0));
    assert_eq!(
        chapters[0].url.as_deref(),
        Some("https://rawkuma.net/manga/alpha/chapter-12/?style=list")
    );
    assert_eq!(chapters[1].chapter_number, Some(11.5));
    assert_eq!(chapters[0].date_uploaded, Some(1_749_472_496_000));

    let pages = parse_pages_html(
        &html::document(include_str!("fixtures/pages.html")),
        "https://rawkuma.net/manga/alpha/chapter-12/?style=list",
        "main .relative section > img",
    )
    .unwrap();
    assert_eq!(pages.len(), 2);
    match &pages[1].content {
        PageContent::Url { url, context } => {
            assert_eq!(url, "https://rawkuma.net/wp-content/uploads/p2.jpg");
            assert_eq!(
                context.as_ref().unwrap()["Referer"],
                "https://rawkuma.net/manga/alpha/chapter-12/?style=list"
            );
            assert_eq!(context.as_ref().unwrap()["User-Agent"], BROWSER_USER_AGENT);
            assert_eq!(pages[1].headers["User-Agent"], BROWSER_USER_AGENT);
        }
        _ => panic!("expected URL page"),
    }
}

#[test]
fn rejects_malformed_payloads() {
    assert!(extract_nonce("<html></html>").is_none());
    assert!(parse_rest_manga_list("{", "https://rawkuma.net", "ja", true).is_err());
    assert!(
        parse_chapters_html(
            &html::document("<div><a href='/manga/alpha/ch-1/'><time datetime='2025-01-01T00:00:00Z'></time></a></div>"),
            "https://rawkuma.net/manga/alpha/",
            "div a",
            "span",
            "time",
            "datetime",
            "ja",
        )
        .is_err()
    );
    assert!(parse_pages_html(
        &html::document("<main><div class='relative'><section><img /></section></div></main>"),
        "https://rawkuma.net/manga/alpha/chapter-12/?style=list",
        "main .relative section > img",
    )
    .unwrap()
    .is_empty());
}

#[test]
fn extracts_gallery_manga_id() {
    let document =
        html::document("<div id='gallery-list' hx-get='/reader?manga_id=321&page=1'></div>");
    assert_eq!(
        extract_manga_id_from_detail_page(&document)
            .unwrap()
            .as_deref(),
        Some("321")
    );
}
