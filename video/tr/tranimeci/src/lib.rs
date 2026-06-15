use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<TRAnimeCI> = AnimeStreamSource::new();

struct TRAnimeCI;

impl AnimeStreamConfig for TRAnimeCI {
    const NAME: &'static str = "TRAnimeCI";
    const BASE_URL: &'static str = "https://tranimaci.com";
    const LANG: &'static str = "tr";
    const LIST_PATH: &'static str = "search";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
