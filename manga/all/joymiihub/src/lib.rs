use manatan_extension::export_manga_source;
use manatan_shared::{impl_masonry_source, manga};
use serde_json::Value;

const SOURCE: JoymiiHub = JoymiiHub;
const CONFIG: manga::MasonryConfig = manga::MasonryConfig {
    base_url: "https://www.joymiihub.com",
    name: "Joymii Hub",
    lang: "all",
    content_rating: "adult",
};

struct JoymiiHub;

impl manga::MasonrySource for JoymiiHub {
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

impl_masonry_source!(JoymiiHub);

export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<figure class="list-gallery"><a href="https://www.joymiihub.com/gallery/sample/" title="Sample Gallery"><img src="https://cdn.joymiihub.com/thumb.jpg"></a></figure>
<ul class="pagination-a"><li class="next"><a href="/archive/page/2/">Next</a></li></ul>
"#;

const DETAILS_FIXTURE: &str = r#"
<meta property="og:title" content="Sample Gallery">
<p class="link-btn"><a href="/model/sample-model/">Sample Model</a><a href="/tag/outdoor/">Outdoor</a></p>
<div id="content"><p>Gallery description.</p></div>
"#;

const PAGES_FIXTURE: &str = r#"
<div class="list-gallery"><a href="https://cdn.joymiihub.com/images/1.jpg">1</a><a href="https://cdn.joymiihub.com/images/2.jpg">2</a></div>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_masonry_source() {
        assert_eq!(manga::Masonry::parse_listing(LIST_FIXTURE, &CONFIG).len(), 1);
        assert_eq!(manga::Masonry::parse_pages(PAGES_FIXTURE, &CONFIG).len(), 2);
    }
}
