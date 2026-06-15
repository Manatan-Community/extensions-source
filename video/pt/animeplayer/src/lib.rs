use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<AnimePlayer> = DooPlaySource::new();

struct AnimePlayer;

impl DooPlayConfig for AnimePlayer {
    const NAME: &'static str = "AnimePlayer";
    const BASE_URL: &'static str = "https://animeplayer.com.br";
    const LANG: &'static str = "pt-BR";
    const POPULAR_PATH: &'static str = "animes";
    const LATEST_PATH: &'static str = "episodios";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
