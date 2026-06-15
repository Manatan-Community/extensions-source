use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<FlixLatam> = DooPlaySource::new();

struct FlixLatam;

impl DooPlayConfig for FlixLatam {
    const NAME: &'static str = "FlixLatam";
    const BASE_URL: &'static str = "https://flixlatam.com";
    const LANG: &'static str = "es";
    const LATEST_PATH: &'static str = "lanzamiento/2024";
    const POPULAR_PATH: &'static str = "pelicula/page";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
