const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://golgebahcesi.com",
    name: "Golge Bahcesi",
    lang: "tr",
    content_rating: "safe",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("themesia_impl.rs");
