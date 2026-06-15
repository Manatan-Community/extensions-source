use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimesDrive> = DooPlaySource::new();

struct AnimesDrive;

impl DooPlayConfig for AnimesDrive {
    const NAME: &'static str = "AnimesDrive";
    const BASE_URL: &'static str = "https://animesdrive.online";
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "adult";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
