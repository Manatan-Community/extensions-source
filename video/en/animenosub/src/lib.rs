use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<Animenosub> = AnimeStreamSource::new();

struct Animenosub;

impl AnimeStreamConfig for Animenosub {
    const NAME: &'static str = "Animenosub";
    const BASE_URL: &'static str = "https://animenosub.to";
    const CONTENT_RATING: &'static str = "adult";
}

export_video_source!(SOURCE);
