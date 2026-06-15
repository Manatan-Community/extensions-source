const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://www.tarotscans.com",
    name: "Tarot Scans",
    lang: "tr",
    content_rating: "safe",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../../id/mangasusu/src/themesia_impl.rs");
