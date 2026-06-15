use manatan_extension::export_manga_source;
use manatan_shared::manga::sinmh::{DetailsStyle, SinmhConfig, SinmhSource};

const SOURCE: SinmhSource<JiuerManhua> = SinmhSource::new();

struct JiuerManhua;

impl SinmhConfig for JiuerManhua {
    const NAME: &'static str = "92Manhua";
    const BASE_URL: &'static str = "http://www.92mh.com";
    const CONTENT_RATING: &'static str = "adult";
    const DETAILS_STYLE: DetailsStyle = DetailsStyle::Dmzj;

    fn chapter_url(path: &str) -> String {
        format!("{}{}", Self::BASE_URL, path)
    }
}

export_manga_source!(SOURCE);
