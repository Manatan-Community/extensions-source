use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_pizza_reader_source,
    manga::{PizzaReaderConfig, PizzaReaderSource},
};

const SOURCE: DdtTeam = DdtTeam;

struct DdtTeam;

impl PizzaReaderSource for DdtTeam {
    fn pizza_config(&self) -> PizzaReaderConfig {
        PizzaReaderConfig {
            base_url: "https://ddt.hastateam.com",
            name: "DDT Team",
            lang: "it",
            content_rating: "safe",
            api_path: "/api",
        }
    }

    fn pizza_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn pizza_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn pizza_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl_pizza_reader_source!(DdtTeam);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"comics":[{"title":"Sample Manga","thumbnail":"/cover.jpg","url":"/comics/sample","last_chapter":{"full_title":"Chapter 1","published_on":"2024-01-01T00:00:00.000000","url":"/chapters/sample-1"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"comic":{"title":"Sample Manga","author":"Author","artist":"Artist","description":"Description","genres":[{"name":"Action"}],"status":"In corso","thumbnail":"/cover.jpg","url":"/comics/sample","chapters":[{"chapter":1,"full_title":"Chapter 1","pages":["/page1.jpg"],"published_on":"2024-01-01T00:00:00.000000","teams":[{"name":"Team"}],"url":"/chapters/sample-1"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages":["/page1.jpg","/page2.jpg"]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::source::MangaSource;
    use serde_json::json;

    #[test]
    fn parses_pizza_reader_fixture() {
        let listing = SOURCE.list(json!({})).unwrap();
        assert_eq!(listing.entries[0].title, "Sample Manga");
        let chapters = SOURCE.chapters(json!({"manga":"/comics/sample"})).unwrap();
        assert_eq!(chapters[0].title.as_deref(), Some("Chapter 1"));
        let pages = SOURCE.pages(json!({"chapter":"/chapters/sample-1"})).unwrap();
        assert_eq!(pages.len(), 2);
    }
}
