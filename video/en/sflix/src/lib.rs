use manatan_extension::export_video_source;
use manatan_shared::video::dopeflix::{DopeFlixConfig, DopeFlixSource};

const SOURCE: DopeFlixSource<SFlix> = DopeFlixSource::new();

struct SFlix;

impl DopeFlixConfig for SFlix {
    const NAME: &'static str = "SFlix";
    const BASE_URL: &'static str = "https://sflix.to";
}

export_video_source!(SOURCE);
