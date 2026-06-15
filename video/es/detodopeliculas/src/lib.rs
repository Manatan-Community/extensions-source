use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<DeTodoPeliculas> = DooPlaySource::new();

struct DeTodoPeliculas;

impl DooPlayConfig for DeTodoPeliculas {
    const NAME: &'static str = "DeTodo Peliculas";
    const BASE_URL: &'static str = "https://detodopeliculas.nu";
    const LANG: &'static str = "es";
    const LATEST_PATH: &'static str = "peliculas-de-estreno";
    const POPULAR_PATH: &'static str = "novedades/page";
    const RESOLVE_EMBED_PAGE: bool = true;
}

export_video_source!(SOURCE);
