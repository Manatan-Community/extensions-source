use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_pizza_reader_source,
    manga::{PizzaReaderConfig, PizzaReaderSource},
};

const SOURCE: PhoenixScans = PhoenixScans;

struct PhoenixScans;

impl PizzaReaderSource for PhoenixScans {
    fn pizza_config(&self) -> PizzaReaderConfig {
        PizzaReaderConfig {
            base_url: "https://www.phoenixscans.com",
            name: "Phoenix Scans",
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

impl_pizza_reader_source!(PhoenixScans);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"comics":[{"title":"Sample Manga","thumbnail":"/cover.jpg","url":"/comics/sample","last_chapter":{"full_title":"Chapter 1","published_on":"2024-01-01T00:00:00.000000","url":"/chapters/sample-1"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"comic":{"title":"Sample Manga","author":"Author","artist":"Artist","description":"Description","genres":[{"name":"Action"}],"status":"In corso","thumbnail":"/cover.jpg","url":"/comics/sample","chapters":[{"chapter":1,"full_title":"Chapter 1","pages":["/page1.jpg"],"published_on":"2024-01-01T00:00:00.000000","teams":[{"name":"Team"}],"url":"/chapters/sample-1"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages":["/page1.jpg","/page2.jpg"]}}"#;
