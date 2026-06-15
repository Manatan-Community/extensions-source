use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<AnimeYtEs> = AnimeStreamSource::new();

struct AnimeYtEs;

impl AnimeStreamConfig for AnimeYtEs {
    const NAME: &'static str = "AnimeYT.es";
    const BASE_URL: &'static str = "https://wwv.animeytx.net";
    const LANG: &'static str = "es";
    const LIST_PATH: &'static str = "tv";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
