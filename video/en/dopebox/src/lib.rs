use manatan_extension::export_video_source;
use manatan_shared::video::dopeflix::{DopeFlixConfig, DopeFlixSource};

const SOURCE: DopeFlixSource<DopeBox> = DopeFlixSource::new();

struct DopeBox;

impl DopeFlixConfig for DopeBox {
    const NAME: &'static str = "DopeBox";
    const BASE_URL: &'static str = "https://dopebox.to";
}

export_video_source!(SOURCE);
