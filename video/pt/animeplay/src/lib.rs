use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimePlay> = DooPlaySource::new();

struct AnimePlay;

impl DooPlayConfig for AnimePlay {
    const NAME: &'static str = "Anime Play";
    const BASE_URL: &'static str = "https://animeplay.cloud";
    const LANG: &'static str = "pt-BR";
    const POPULAR_PATH: &'static str = "anime";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
