const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://doujinmoon.com",
    name: "Doujin Moon",
    lang: "th",
    content_rating: "adult",
    manga_dir: "/series",
};

struct MangaThemesiaSource;

include!("../../../id/mangasusu/src/themesia_impl.rs");
