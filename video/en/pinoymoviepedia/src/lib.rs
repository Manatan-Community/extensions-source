use manatan_extension::export_video_source;
use manatan_shared::video::dooplay::{DooPlayConfig, DooPlaySource};

const SOURCE: DooPlaySource<PinoyMoviePedia> = DooPlaySource::new();

struct PinoyMoviePedia;

impl DooPlayConfig for PinoyMoviePedia {
    const NAME: &'static str = "PinoyMoviePedia";
    const BASE_URL: &'static str = "https://pinoymoviepedia.ru";
    const CONTENT_RATING: &'static str = "adult";
    const LATEST_PATH: &'static str = "ano/2024";
}

export_video_source!(SOURCE);
