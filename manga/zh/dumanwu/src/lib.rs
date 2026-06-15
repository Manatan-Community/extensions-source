use manatan_extension::export_manga_source;
use manatan_shared::mmlook::{MMLookConfig, MMLookSource};

const SOURCE: MMLookSource<DumanwuConfig> = MMLookSource::new();

struct DumanwuConfig;

impl MMLookConfig for DumanwuConfig {
    const NAME: &'static str = "读漫屋";
    const BASE_URL: &'static str = "https://m.dumanwu1.com";
    const DESKTOP_URL: &'static str = "https://www.dumanwu1.com";
    const LANG: &'static str = "zh";
    const CONTENT_RATING: &'static str = "safe";
}

export_manga_source!(SOURCE);
