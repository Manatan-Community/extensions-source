const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://www.manhwaindo.my",
    name: "Manhwa Indo",
    lang: "id",
    content_rating: "adult",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../mangasusu/src/themesia_impl.rs");
