use manatan_extension::export_video_source;
use manatan_shared::video::dopeflix::{DopeFlixConfig, DopeFlixSource};

const SOURCE: DopeFlixSource<DopeFlix> = DopeFlixSource::new();

struct DopeFlix;

impl DopeFlixConfig for DopeFlix {
    const NAME: &'static str = "DopeFlix";
    const BASE_URL: &'static str = "https://dopeflix.to";
}

export_video_source!(SOURCE);
