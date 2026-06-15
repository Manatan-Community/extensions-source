const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://mangakusu.com",
    name: "Manga Kusu",
    lang: "tr",
    content_rating: "safe",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../../id/mangasusu/src/themesia_impl.rs");
