use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<Anikyuu> = AnimeStreamSource::new();

struct Anikyuu;

impl AnimeStreamConfig for Anikyuu {
    const NAME: &'static str = "Anikyuu";
    const BASE_URL: &'static str = "https://anikyuu.to";
    const LANG: &'static str = "pt-BR";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
