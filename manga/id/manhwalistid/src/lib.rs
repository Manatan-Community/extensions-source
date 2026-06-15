const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://manhwalist02.asia",
    name: "Manhwa List",
    lang: "id",
    content_rating: "safe",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../mangasusu/src/themesia_impl.rs");
