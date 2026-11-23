//! Current publisher site. Legacy parsers stay available for migration fixtures.
use super::*;
use manatan_sdk::client::BrowserChallengePolicy;
use serde_json::json;

pub const SITE: &str = "https://witanime.site";

pub fn url(key: &str) -> Result<String> {
    let u = Url::parse(SITE).unwrap().join(key).map_err(url_error)?;
    let parts: Vec<_> = u
        .path_segments()
        .ok_or_else(|| Error::new("invalid WIT ANIME path"))?
        .filter(|x| !x.is_empty())
        .collect();
    if u.scheme() != "https"
        || u.host_str() != Some("witanime.site")
        || u.port().is_some()
        || !u.username().is_empty()
        || u.password().is_some()
        || !matches!(parts.as_slice(), ["anime" | "movie", _] | ["watch", _, _])
        || u.query().is_some()
        || u.fragment().is_some()
    {
        return Err(Error::new("invalid WIT ANIME content key"));
    }
    Ok(u.to_string())
}
fn key(value: &str) -> Option<String> {
    let u = url(value).ok()?;
    Some(format!(
        "{}/",
        Url::parse(&u).ok()?.path().trim_end_matches('/')
    ))
}

impl WitAnime {
    pub fn current_html(&self, target: &str) -> Result<String> {
        // Laravel's anonymous session is established by the public home page.
        let get = |u: &str| {
            Client::browser()
                .get(u)
                .cookies_for(u)
                .header("Referer", SITE)
                .header(
                    "Accept",
                    if u.contains("/new-episodes?") {
                        "application/json"
                    } else {
                        "text/html"
                    },
                )
                .timeout_ms(25_000)
                .max_body_bytes(8 * 1024 * 1024)
                .send_with_challenge(
                    &BrowserChallengePolicy::cloudflare(SITE).profile("witanime-current"),
                )
        };
        let response = get(target)?;
        let response = if response.status() == 403 {
            get(SITE)?.error_for_status()?;
            get(target)?
        } else {
            response
        };
        Ok(response.error_for_status()?.text()?.to_string())
    }
    pub fn current_page(&self, page: u32, query: Option<&str>) -> Result<Paged<CatalogItem>> {
        let path = if page <= 1 {
            "/browse".to_string()
        } else {
            format!("/browse/page/{page}")
        };
        let mut target = Url::parse(SITE).unwrap().join(&path).map_err(url_error)?;
        if let Some(q) = query {
            target.set_path("/search");
            target
                .query_pairs_mut()
                .append_pair("q", q)
                .append_pair("page", &page.max(1).to_string());
        }
        catalog(&self.current_html(target.as_str())?)
    }
    pub fn current_latest(&self, page: u32) -> Result<Paged<CatalogItem>> {
        let response = self.current_extract(
            SITE,
            "document.readyState === 'complete' && !!document.querySelector('main')".into(),
            format!(r#"(async () => {{
                const response = await fetch('/new-episodes?page={}', {{headers: {{Accept: 'application/json'}}}});
                if (!response.ok) throw new Error('WIT ANIME latest HTTP ' + response.status);
                return await response.json();
            }})()"#, page.max(1)),
            vec![],
        )?;
        let value = response
            .value
            .ok_or_else(|| Error::new("WIT ANIME latest response is missing"))?;
        let html = value["html"]
            .as_str()
            .ok_or_else(|| Error::new("WIT ANIME latest response has no episode HTML"))?;
        let mut result = catalog(html)?;
        result.has_next_page = value["hasOlder"].as_bool().unwrap_or(false);
        Ok(result)
    }
    pub fn current_streams(&self, episode: &VideoEpisode) -> Result<Vec<VideoStream>> {
        let target = url(&episode.key)?;
        if !Url::parse(&target).unwrap().path().starts_with("/watch/") {
            return Err(Error::new(
                "WIT ANIME episode key must identify a watch page",
            ));
        }
        let response = self.current_browser(&target, None)?;
        let value = response.value.unwrap_or(Value::Null);
        if value["error"].as_bool() == Some(true) {
            return Err(Error::new(
                "WIT ANIME could not load its current playback server list",
            ));
        }
        let mut streams = Vec::new();
        for (quality, servers) in value["players"].as_object().into_iter().flatten() {
            for server in servers.as_array().into_iter().flatten() {
                let Some(label) = server["label"].as_str() else {
                    continue;
                };
                if !matches!(
                    label.to_ascii_lowercase().as_str(),
                    "videa" | "hgcloud" | "yonaplay" | "mail"
                ) {
                    continue; // Other listed services require extractors this source does not implement.
                }
                // The provider resolves fresh, session-bound tokens only after selection.
                streams.push(VideoStream {
                    name: Some(format!("{label} - {quality}")),
                    quality: Some(quality.clone()),
                    extra: json!({"server":label,"quality":quality,"version":server["version"],"lang":server["lang"]})
                        .as_object().unwrap().clone().into_iter().collect(),
                    ..VideoStream::default()
                });
            }
        }
        streams.sort_by_key(|stream| {
            match stream
                .extra
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "hgcloud" => 0,
                "videa" => 1,
                "yonaplay" => 2,
                _ => 3,
            }
        });
        if streams.is_empty() {
            return Err(Error::new(format!(
                "WIT ANIME returned no playback servers (response fields: {}; quality groups: {})",
                value
                    .as_object()
                    .map(|v| v.keys().cloned().collect::<Vec<_>>().join(","))
                    .unwrap_or_default(),
                value["players"].as_object().map(|v| v.len()).unwrap_or(0)
            )));
        }
        Ok(streams)
    }
    pub fn current_browser(
        &self,
        target: &str,
        selected: Option<&VideoStream>,
    ) -> Result<WebViewExtractResponse> {
        let selection = selected
            .map(|v| serde_json::to_string(&v.extra))
            .transpose()?
            .unwrap_or_else(|| "null".into());
        let wait = format!(
            r#"(() => {{
            const el=document.querySelector('[x-data^="watchPlayer"]');
            if(!el || !window.Alpine) return false;
            const d=Alpine.$data(el);if(!d.sourcesLoaded)return false;
            const selection={selection};if(!selection||d.sourcesError)return true;
            if(!window.__manatanSelected){{
                window.__manatanSelected=true;
                d.selectedQuality=selection.quality;d.selectedVersion=selection.version||'sub';d.selectedLang=selection.lang||'jp';
                d.selectedServer=d.currentServers.findIndex(s=>s.label===selection.server);
                if(d.selectedServer<0)return true;d.playerStart();
            }}
            if(d.sourceError)return true;
            const gate=el.querySelector('[data-player-frame] iframe');
            if(gate && !d.frameLoading)return true;
            try {{if(gate?.contentDocument?.querySelector('iframe[src]'))return true;}}catch(_){{}}
            return false;
        }})()"#
        );
        self.current_extract(target, wait, r#"(() => {
                const el=document.querySelector('[x-data^="watchPlayer"]');
                const d=el&&window.Alpine?Alpine.$data(el):null;
                const gate=el?.querySelector('[data-player-frame] iframe');
                const embeds=[];
                try {for(const frame of gate?.contentDocument?.querySelectorAll('iframe[src]')||[])embeds.push(frame.src);}catch(_){}
                if(gate?.src)embeds.push(gate.src);
                return {players:d?.players||{},error:d?.sourcesError||d?.sourceError||false,embeds};
            })()"#.into(), vec![capture("videa.hu"),capture("yonaplay.net"),capture("hgcloud.to"),capture("audinifer.com"),capture(".m3u8"),capture(".mp4")])
    }
    fn current_extract(
        &self,
        target: &str,
        wait: String,
        script: String,
        capture_requests: Vec<WebViewRequestCapture>,
    ) -> Result<WebViewExtractResponse> {
        browser::extract(&WebViewExtractRequest {
            url: target.into(),
            cookie_url: Some(SITE.into()),
            session: Some(WebViewSession {
                id: "witanime-current".into(),
                persistence: WebViewSessionPersistence::Persistent,
                ..Default::default()
            }),
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_script: Some(wait),
            script,
            timeout_ms: Some(45_000),
            headless: Some(true),
            cookies: true,
            method: Default::default(),
            body: None,
            headers: vec![],
            user_agent: None,
            wait_for_selector: None,
            wait_for_event: None,
            preload_scripts: vec![],
            capture_requests,
            capture_events: vec![],
        })
    }
}

pub fn catalog(body: &str) -> Result<Paged<CatalogItem>> {
    let doc = Html::parse_document(body);
    let mut seen = BTreeSet::new();
    let mut items = Vec::new();
    for a in doc.select(&selector("a[data-preview], a[href*=\"/watch/\"]")?) {
        let Some(mut item_key) = a.value().attr("href").and_then(key) else {
            continue;
        };
        if item_key.starts_with("/watch/") {
            let slug = item_key.split('/').nth(2).unwrap_or_default();
            item_key = format!("/anime/{slug}/");
        }
        if !seen.insert(item_key.clone()) {
            continue;
        }
        let title = a
            .select(&selector("img[alt]")?)
            .filter_map(|e| e.value().attr("alt"))
            .find(|s| !s.trim().is_empty())
            .map(clean)
            .unwrap_or_else(|| clean(&a.text().collect::<String>()));
        if title.is_empty() {
            continue;
        }
        let cover = a
            .select(&selector("img[src]")?)
            .next()
            .and_then(|i| i.value().attr("src"))
            .and_then(cover);
        items.push(CatalogItem {
            key: item_key.clone(),
            title,
            url: Some(format!("{SITE}{item_key}")),
            cover,
            language: Some("ar".into()),
            ..Default::default()
        });
    }
    if items.is_empty() && !body.contains("لا توجد نتائج") {
        return Err(Error::new(
            "WIT ANIME returned no recognizable catalog cards",
        ));
    }
    Ok(Paged::new(
        items,
        doc.select(&selector("a[rel=next]")?).next().is_some(),
    ))
}
fn cover(value: &str) -> Option<ImageRequest> {
    let u = Url::parse(value).ok()?;
    if u.scheme() != "https" || !matches!(u.host_str()?, "images.witanime.site" | "witanime.site") {
        return None;
    }
    Some(
        ImageRequest::get(value)
            .header("Referer", SITE)
            .cookies_for(value),
    )
}
pub fn details(body: &str, item: &CatalogItem) -> Result<CatalogItem> {
    let doc = Html::parse_document(body);
    let data = doc
        .select(&selector("script[type=\"application/ld+json\"]")?)
        .filter_map(|s| serde_json::from_str::<Value>(&s.inner_html()).ok())
        .find(|v| matches!(v["@type"].as_str(), Some("TVSeries" | "Movie")))
        .ok_or_else(|| Error::new("WIT ANIME title metadata is missing"))?;
    let mut result = item.clone();
    result.title = data["name"]
        .as_str()
        .ok_or_else(|| Error::new("WIT ANIME title is missing"))?
        .into();
    result.description = data["description"].as_str().map(str::to_string);
    result.cover = data["image"].as_str().and_then(cover);
    result.tags = data["genre"]
        .as_array()
        .map(|v| {
            v.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    result.url = Some(url(&item.key)?);
    result.initialized = true;
    Ok(result)
}
pub fn episodes(body: &str) -> Result<Vec<VideoEpisode>> {
    let doc = Html::parse_document(body);
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for a in doc.select(&selector("[data-episode-list] a[href*=\"/watch/\"]")?) {
        let Some(k) = a.value().attr("href").and_then(key) else {
            continue;
        };
        if !seen.insert(k.clone()) {
            continue;
        }
        let n = k
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|x| x.parse::<f32>().ok());
        rows.push(VideoEpisode {
            key: k,
            title: Some(clean(&a.text().collect::<String>())),
            episode_number: n,
            ..Default::default()
        });
    }
    rows.sort_by(|a, b| {
        b.episode_number
            .partial_cmp(&a.episode_number)
            .unwrap_or(Ordering::Equal)
    });
    if rows.is_empty() {
        return Err(Error::new("WIT ANIME title has no published episodes"));
    }
    Ok(rows)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn current_catalog_and_episode_keys_are_distinct_and_validated() {
        let h = r#"<a data-preview="/preview/anime/one-piece" href="https://witanime.site/anime/one-piece"><img src="https://images.witanime.site/a.jpg" alt="One Piece"></a><a rel="next" href="/browse/page/2">Next</a><div data-episode-list><a href="/watch/one-piece/1">Episode 1</a><a href="/watch/one-piece/2">Episode 2</a><a href="/watch/one-piece/2">Duplicate</a></div>"#;
        let c = catalog(h).unwrap();
        assert_eq!(c.entries.len(), 1);
        assert!(c.has_next_page);
        let e = episodes(h).unwrap();
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].episode_number, Some(2.));
        assert!(url("https://evil.test/anime/one-piece").is_err());
        assert!(url("https://witanime.site:8443/watch/a/1").is_err());
    }
}
