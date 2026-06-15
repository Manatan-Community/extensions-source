use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<AnimeBalkan> = AnimeStreamSource::new();

struct AnimeBalkan;

impl AnimeStreamConfig for AnimeBalkan {
    const NAME: &'static str = "AnimeBalkan";
    const BASE_URL: &'static str = "https://animebalkan.org";
    const LANG: &'static str = "sr";
    const LIST_PATH: &'static str = "animesaprevodom";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
