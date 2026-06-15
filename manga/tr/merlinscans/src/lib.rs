const SOURCE: Source = Source;
const BASE_URL: &str = "https://merlintoon.com";
const NAME: &str = "Merlin Scans";
const LANG: &str = "tr";
const CONTENT_RATING: &str = "safe";
const POPULAR_SLUG: &str = "seri-siralamasi";
const LATEST_SLUG: &str = "son-guncellenenler";

struct Source;

include!("initmanga_impl.rs");
