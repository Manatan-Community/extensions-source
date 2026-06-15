const SOURCE: Source = Source;
const BASE_URL: &str = "https://ragnarscans.com";
const NAME: &str = "Ragnar Scans";
const LANG: &str = "tr";
const CONTENT_RATING: &str = "safe";
const POPULAR_SLUG: &str = "en-cok-takip-edilenler";
const LATEST_SLUG: &str = "recently-updated";

struct Source;

include!("../../merlinscans/src/initmanga_impl.rs");
