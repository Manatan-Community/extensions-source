use manatan_extension::export_manga_source;
use manatan_shared::mangotheme::{MangoThemeConfig, MangoThemeSource};

const SOURCE: MangoThemeSource<MangoToons> = MangoThemeSource::new();

struct MangoToons;

impl MangoThemeConfig for MangoToons {
    const NAME: &'static str = "Mango Toons";
    const BASE_URL: &'static str = "https://mangotoons.com";
    const API_URL: &'static str = "https://mangotoons.com/api";
    const CDN_URL: &'static str = "https://cdn.mangotoons.com";
    const LANG: &'static str = "pt-BR";
    const CONTENT_RATING: &'static str = "safe";
    const ENCRYPTION_KEY: &'static str = "abmPisXlFjOLVTnYhbYQTpkWJtOGKwVttzLqstfjRBNVaEtQYG";
    const REQUIRES_LOGIN: bool = true;
}

export_manga_source!(SOURCE);
