use manatan_extension::export_manga_source;
use manatan_shared::{impl_masonry_source, manga};
use serde_json::Value;

const SOURCE: MetartHunter = MetartHunter;
const CONFIG: manga::MasonryConfig = manga::MasonryConfig {
    base_url: "https://www.metarthunter.com",
    name: "Metart Hunter",
    lang: "all",
    content_rating: "adult",
};

struct MetartHunter;

impl manga::MasonrySource for MetartHunter {
    fn masonry_config(&self, _request: &Value) -> &manga::MasonryConfig {
        &CONFIG
    }

    fn masonry_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn masonry_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn masonry_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl_masonry_source!(MetartHunter);

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<figure class="list-gallery"><a href="https://www.metarthunter.com/gallery/sample/" title="Sample Gallery"><img src="/thumb.jpg"></a></figure>
<ul class="pagination-a"><li class="next"><a href="/archive/page/2/">Next</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample Gallery">
<p class="link-btn"><a href="/model/sample-model/">Sample Model</a><a href="/tag/outdoor/">Outdoor</a></p>
<div id="content"><p>Gallery description.</p></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="list-gallery"><a href="https://cdn.metarthunter.com/images/1.jpg">1</a><a href="https://cdn.metarthunter.com/images/2.jpg">2</a></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_masonry_source() {
        assert_eq!(manga::Masonry::parse_listing(LIST_FIXTURE, &CONFIG).len(), 1);
        assert!(manga::Masonry::has_next_page(LIST_FIXTURE));
        assert_eq!(
            manga::Masonry::parse_details(DETAILS_FIXTURE, Some("/gallery/sample".into()), &CONFIG)
                .title,
            "Sample Gallery"
        );
        assert_eq!(manga::Masonry::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 2);
    }
}
