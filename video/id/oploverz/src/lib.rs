use manatan_extension::export_video_source;
use manatan_shared::{
    url,
    video::indonesian::{IndonesianConfig, IndonesianSource},
};

const SOURCE: IndonesianSource<Oploverz> = IndonesianSource::new();

struct Oploverz;

impl IndonesianConfig for Oploverz {
    const NAME: &'static str = "Oploverz";
    const BASE_URL: &'static str = "https://oploverz.media";
    const QUALITY_DEFAULT: &'static str = "720p";

    fn list_url(listing: &str, page: u64) -> String {
        let order = if listing == "latest" {
            "latest"
        } else {
            "popular"
        };
        format!("{}/anime-list/page/{page}/?order={order}", Self::BASE_URL)
    }

    fn search_url(query: &str, page: u64, genre: Option<String>) -> String {
        let mut params = vec![format!("title={}", url::query_escape(query))];
        if let Some(genre) = genre.filter(|value| !value.is_empty()) {
            params.push(format!("genre[]={}", url::query_escape(&genre)));
        }
        format!(
            "{}/anime-list/page/{page}/?{}",
            Self::BASE_URL,
            params.join("&")
        )
    }
}

export_video_source!(SOURCE);
