const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://www.tanuki-manga.net",
    name: "Tanuki-Manga",
    lang: "th",
    content_rating: "adult",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../../id/mangasusu/src/themesia_impl.rs");
