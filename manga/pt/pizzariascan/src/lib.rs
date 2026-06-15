use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_pizza_reader_source,
    manga::{PizzaReaderConfig, PizzaReaderSource},
};

const SOURCE: PizzariaScan = PizzariaScan;

struct PizzariaScan;

impl PizzaReaderSource for PizzariaScan {
    fn pizza_config(&self) -> PizzaReaderConfig {
        PizzaReaderConfig {
            base_url: "https://pizzariacomics.com",
            name: "PizzariaScan",
            lang: "pt-BR",
            content_rating: "adult",
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

impl_pizza_reader_source!(PizzariaScan);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"{"comics":[{"title":"Quadrinho de exemplo","thumbnail":"/cover.jpg","url":"/comics/exemplo","status":"Em andamento","last_chapter":{"full_title":"Capitulo 1","published_on":"2024-01-01T00:00:00.000000","url":"/chapters/exemplo-1"}}]}"#;
const DETAILS_FIXTURE: &str = r#"{"comic":{"title":"Quadrinho de exemplo","author":"Autor","artist":"Artista","description":"Descricao","genres":[{"name":"Acao"}],"status":"Em andamento","thumbnail":"/cover.jpg","url":"/comics/exemplo","chapters":[{"chapter":1,"full_title":"Capitulo 1","pages":["/page1.jpg"],"published_on":"2024-01-01T00:00:00.000000","teams":[{"name":"PizzariaScan"}],"url":"/chapters/exemplo-1"}]}}"#;
const PAGES_FIXTURE: &str = r#"{"chapter":{"pages":["/page1.jpg","/page2.jpg"]}}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use manatan_extension::source::MangaSource;
    use serde_json::json;

    #[test]
    fn parses_pizza_reader_fixtures() {
        assert_eq!(SOURCE.list(json!({"listingId":"popular"})).unwrap().entries.len(), 1);
        assert_eq!(SOURCE.chapters(json!({"manga":"/comics/exemplo"})).unwrap().len(), 1);
        assert_eq!(SOURCE.pages(json!({"chapter":"/chapters/exemplo-1"})).unwrap().len(), 2);
    }
}
