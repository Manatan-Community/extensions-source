use manatan_extension::export_manga_source;
use manatan_shared::{
    impl_gattsu_source,
    manga::{GattsuConfig, GattsuSource},
};

const SOURCE: HentaiSeason = HentaiSeason;
const BASE_URL: &str = "https://hentaiseason.com";

struct HentaiSeason;

impl GattsuSource for HentaiSeason {
    fn gattsu_config(&self) -> GattsuConfig {
        GattsuConfig {
            base_url: BASE_URL,
            name: "Hentai Season",
            lang: "pt-BR",
            content_rating: "adult",
        }
    }

    fn gattsu_list_fixture(&self) -> &'static str {
        LIST_FIXTURE
    }

    fn gattsu_details_fixture(&self) -> &'static str {
        DETAILS_FIXTURE
    }

    fn gattsu_pages_fixture(&self) -> &'static str {
        PAGES_FIXTURE
    }
}

impl_gattsu_source!(HentaiSeason);
export_manga_source!(SOURCE);

const LIST_FIXTURE: &str = r#"
<div class="meio"><div class="lista"><ul>
  <li><a href="https://hentaiseason.com/sample-post/"><span class="thumb-imagem"><img class="wp-post-image" src="/thumb-300x300.jpg"></span><span class="thumb-titulo">Sample Hentai Season</span></a></li>
</ul></div></div>
"#;

const DETAILS_FIXTURE: &str = r#"
<div class="meio"><div class="post-box">
  <h1 class="post-titulo">Sample Hentai Season</h1>
  <div class="post-capa"><img class="wp-post-image" src="/cover-300x300.jpg"></div>
  <ul class="post-itens"><li>Artista: <a>Artist</a></li><li>Tags: <a>Drama</a></li></ul>
  <div class="post-texto"><p>Sinopse : Sample description.</p></div>
  <ul class="post-fotos"><li><a><img src="/page-1.jpg"></a></li></ul>
</div></div>
"#;

const PAGES_FIXTURE: &str = DETAILS_FIXTURE;
