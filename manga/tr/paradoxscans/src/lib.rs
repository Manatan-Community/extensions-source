const SOURCE: Source = Source;
const BASE_URL: &str = "https://paradoxscans.com";
const NAME: &str = "Paradox Scans";
const LANG: &str = "tr";
const CONTENT_RATING: &str = "safe";
const POPULAR_SLUG: &str = "manga-ranking";
const LATEST_SLUG: &str = "recently-updated";

struct Source;

include!("../../merlinscans/src/initmanga_impl.rs");
