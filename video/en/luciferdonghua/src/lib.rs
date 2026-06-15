use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<LuciferDonghua> = AnimeStreamSource::new();

struct LuciferDonghua;

impl AnimeStreamConfig for LuciferDonghua {
    const NAME: &'static str = "LuciferDonghua";
    const BASE_URL: &'static str = "https://luciferdonghua.in";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
