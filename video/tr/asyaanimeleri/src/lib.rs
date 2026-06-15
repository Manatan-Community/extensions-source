use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<AsyaAnimeleri> = AnimeStreamSource::new();

struct AsyaAnimeleri;

impl AnimeStreamConfig for AsyaAnimeleri {
    const NAME: &'static str = "AsyaAnimeleri";
    const BASE_URL: &'static str = "https://asyaanimeleri.top";
    const LANG: &'static str = "tr";
    const LIST_PATH: &'static str = "series";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
