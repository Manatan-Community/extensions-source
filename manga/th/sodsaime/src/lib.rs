const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://www.xn--l3c0azab5a2gta.com",
    name: "สดใสเมะ",
    lang: "th",
    content_rating: "adult",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../../id/mangasusu/src/themesia_impl.rs");
