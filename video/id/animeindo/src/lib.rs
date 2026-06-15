use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<AnimeIndo> = IndonesianSource::new();

struct AnimeIndo;

impl IndonesianConfig for AnimeIndo {
    const NAME: &'static str = "AnimeIndo";
    const BASE_URL: &'static str = "https://animeindo.skin";
    const QUALITY_DEFAULT: &'static str = "720p";

    fn list_url(listing: &str, page: u64) -> String {
        let sort = if listing == "latest" {
            "created_at"
        } else {
            "view"
        };
        format!("{}/browse?sort={sort}&page={page}", Self::BASE_URL)
    }

    fn search_url(query: &str, page: u64, genre: Option<String>) -> String {
        let mut params = vec![format!("page={page}")];
        if !query.is_empty() {
            params.push(format!("title={}", url::query_escape(query)));
        }
        if let Some(genre) = genre.filter(|value| !value.is_empty()) {
            params.push(format!("genre[]={}", url::query_escape(&genre)));
        }
        format!("{}/browse?{}", Self::BASE_URL, params.join("&"))
    }
}

export_video_source!(SOURCE);
