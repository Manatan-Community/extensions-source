const SOURCE: MangaThemesiaSource = MangaThemesiaSource;
const CONFIG: ThemesiaConfig = ThemesiaConfig {
    base_url: "https://02.manhwaland.land",
    name: "ManhwaLand.mom",
    lang: "id",
    content_rating: "adult",
    manga_dir: "/manga",
};

struct MangaThemesiaSource;

include!("../../mangasusu/src/themesia_impl.rs");
