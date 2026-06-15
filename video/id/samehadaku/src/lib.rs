use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<Samehadaku> = IndonesianSource::new();

struct Samehadaku;

impl IndonesianConfig for Samehadaku {
    const NAME: &'static str = "Samehadaku";
    const BASE_URL: &'static str = "https://v2.samehadaku.how";
    const QUALITY_DEFAULT: &'static str = "720p";

    fn list_url(listing: &str, page: u64) -> String {
        let order = if listing == "latest" {
            "update"
        } else {
            "popular"
        };
        format!("{}/daftar-anime-2/page/{page}/?order={order}", Self::BASE_URL)
    }

    fn search_url(query: &str, page: u64, genre: Option<String>) -> String {
        let mut params = vec![format!("title={}", url::query_escape(query))];
        if let Some(genre) = genre.filter(|value| !value.is_empty()) {
            params.push(format!("genre[]={}", url::query_escape(&genre)));
        }
        format!(
            "{}/daftar-anime-2/page/{page}/?{}",
            Self::BASE_URL,
            params.join("&")
        )
    }
}

export_video_source!(SOURCE);
