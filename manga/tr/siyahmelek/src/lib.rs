const SOURCE: Source = Source;
const BASE_URL: &str = "https://siyahmelek.vip";
const NAME: &str = "Siyah Melek";
const LANG: &str = "tr";
const CONTENT_RATING: &str = "adult";
const POPULAR_SLUG: &str = "trending-manga";
const LATEST_SLUG: &str = "recently-updated";

struct Source;

include!("../../merlinscans/src/initmanga_impl.rs");
