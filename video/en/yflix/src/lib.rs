use manatan_extension::export_video_source;
use manatan_shared::video::yflix::{YFlixConfig, YFlixSource};

const SOURCE: YFlixSource<YFlix> = YFlixSource::new();

struct YFlix;

impl YFlixConfig for YFlix {
    const NAME: &'static str = "YFlix";
    const BASE_URL: &'static str = "https://yflix.to";
}

export_video_source!(SOURCE);
