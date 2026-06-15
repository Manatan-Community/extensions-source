const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://arcurafansub.com",
    name: "Arcura Fansub",
    lang: "tr",
    content_rating: "adult",
    manga_dir: "/seri",
};

struct MangaThemesiaSource;

include!("../../../id/mangasusu/src/themesia_impl.rs");
