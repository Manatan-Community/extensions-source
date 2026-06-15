use manatan_extension::export_video_source;
use manatan_shared::video::yflix::{YFlixConfig, YFlixSource};

const SOURCE: YFlixSource<MovHub> = YFlixSource::new();

struct MovHub;

impl YFlixConfig for MovHub {
    const NAME: &'static str = "MovHub";
    const BASE_URL: &'static str = "https://1moviesz.to";
}

export_video_source!(SOURCE);
