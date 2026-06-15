use manatan_extension::export_manga_source;
use manatan_shared::mmlook::{MMLookConfig, MMLookSource};

const SOURCE: MMLookSource<RumanhuaConfig> = MMLookSource::new();

struct RumanhuaConfig;

impl MMLookConfig for RumanhuaConfig {
    const NAME: &'static str = "如漫画";
    const BASE_URL: &'static str = "https://m.rumanhua2.com";
    const DESKTOP_URL: &'static str = "https://www.rumanhua2.com";
    const LANG: &'static str = "zh";
    const CONTENT_RATING: &'static str = "safe";
    const USE_LEGACY_MANGA_URL: bool = true;
}

export_manga_source!(SOURCE);
