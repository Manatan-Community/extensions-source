use manatan_extension::export_video_source;
use manatan_shared::video::animestream::{AnimeStreamConfig, AnimeStreamSource};

const SOURCE: AnimeStreamSource<Animeito> = AnimeStreamSource::new();

struct Animeito;

impl AnimeStreamConfig for Animeito {
    const NAME: &'static str = "Animeito";
    const BASE_URL: &'static str = "https://animesonline.io";
    const LANG: &'static str = "pt-BR";
    const QUALITY_DEFAULT: &'static str = "1080p";
}

export_video_source!(SOURCE);
