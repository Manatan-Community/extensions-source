use manatan_extension::export_manga_source;
use manatan_shared::manga::sinmh::{DetailsStyle, SinmhConfig, SinmhSource};

const SOURCE: SinmhSource<Ykmh> = SinmhSource::new();

struct Ykmh;

impl SinmhConfig for Ykmh {
    const NAME: &'static str = "YKMH";
    const BASE_URL: &'static str = "https://www.ykmh.net";
    const CONTENT_RATING: &'static str = "adult";
    const DETAILS_STYLE: DetailsStyle = DetailsStyle::Dmzj;
    const KEEP_CHAPTER_ORDER: bool = true;
}

export_manga_source!(SOURCE);
