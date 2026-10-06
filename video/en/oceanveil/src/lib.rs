use manatan_sdk::{
    browser::{
        self, WebViewExtractRequest, WebViewExtractResponse, WebViewRequestCapture, WebViewSession,
        WebViewWaitUntil,
    },
    client::Client,
    model::{
        AuthenticationAction, AuthenticationRequest, AuthenticationState,
        AuthenticationWebViewCompletion, CatalogItem, ImageRequest, Paged, UrlResolveResult,
        VideoEpisode, VideoStream,
    },
    Error, Result, VideoSource,
};
use serde_json::{json, Value};
use url::Url;
const BASE: &str = "https://oceanveil.net";
const MEDIA: &str = "https://anime-contents-stream.oceanveil.net";
const IMAGE: &str = "https://image.oceanveil.net/public";
pub struct OceanVeil {
    client: Client,
}
impl Default for OceanVeil {
    fn default() -> Self {
        Self {
            client: Client::browser().cookies_for(BASE),
        }
    }
}
fn numeric(value: &str) -> Result<u64> {
    value
        .parse::<u64>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| Error::new("Invalid OceanVeil content identifier"))
}
fn rows(data: &Value) -> Result<&Vec<Value>> {
    data["data"]
        .as_array()
        .ok_or_else(|| Error::new("OceanVeil catalog format changed"))
}
fn attributes(row: &Value) -> Result<&Value> {
    row.get("attributes")
        .filter(|v| v.is_object())
        .ok_or_else(|| Error::new("OceanVeil attributes are missing"))
}
fn id(row: &Value) -> Result<&str> {
    let id = row["id"]
        .as_str()
        .ok_or_else(|| Error::new("OceanVeil content identifier is missing"))?;
    numeric(id)?;
    Ok(id)
}
fn cover_path(id: &str) -> Result<String> {
    let n = numeric(id)?;
    let padded = format!("{n:09}");
    Ok(format!(
        "{IMAGE}/anime_titles/{}/{}/{padded}/{padded}_vertical_with_logo.jpg_small.webp",
        &padded[..3],
        &padded[..6]
    ))
}
fn title(row: &Value, data: &Value) -> Result<CatalogItem> {
    let id = id(row)?;
    let a = attributes(row)?;
    let name = a["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new("OceanVeil title is missing"))?;
    let mut i = CatalogItem::new(id, name);
    i.url = Some(format!("{BASE}/anime_titles/{id}"));
    i.language = Some("en".into());
    i.initialized = true;
    i.cover = Some(ImageRequest::get(cover_path(id)?));
    i.description = a["summary"].as_str().map(str::to_owned);
    i.status = Some(json!(if a["isCompleted"].as_bool() == Some(true) {
        "completed"
    } else {
        "ongoing"
    }));
    i.content_rating = Some(
        if a["isMature"].as_bool() == Some(true) {
            "adult"
        } else {
            "general"
        }
        .into(),
    );
    if let Some(included) = data["included"].as_array() {
        for relation in ["genre", "tags"] {
            let refs = &row["relationships"][relation]["data"];
            let ids: Vec<_> = if let Some(a) = refs.as_array() {
                a.iter().collect()
            } else {
                vec![refs]
            };
            for r in ids {
                if let Some(entity) = included
                    .iter()
                    .find(|v| v["type"] == r["type"] && v["id"] == r["id"])
                {
                    if let Some(name) = entity["attributes"]["name"].as_str() {
                        i.tags.push(name.into())
                    }
                }
            }
        }
    }
    i.extra
        .insert("drmRequired".into(), a["isDrmRequired"].clone());
    i.extra.insert(
        "accessibleInCountry".into(),
        a["isAccessibleInCountry"].clone(),
    );
    Ok(i)
}
fn login_action() -> AuthenticationState {
    AuthenticationState{message:Some("Sign in to OceanVeil on its website. Subscription and regional restrictions still apply; DRM-protected titles require the official player.".into()),action:Some(AuthenticationAction::WebView{url:format!("{BASE}/user/login"),profile:"oceanveil".into(),cookie_url:Some(BASE.into()),completion:AuthenticationWebViewCompletion{required_cookie_names:vec!["auth._token.local".into()],..Default::default()}}),..Default::default()}
}
fn player_request(player_url: String) -> WebViewExtractRequest {
    WebViewExtractRequest {
            url: player_url,
            cookie_url: Some(BASE.into()),
            session: Some(WebViewSession { id: "oceanveil".into(), ..Default::default() }),
            wait_until: Some(WebViewWaitUntil::DomReady),
            wait_for_script: Some(r"performance.getEntriesByType('resource').some(r => /\/v1\/video\.m3u8(?:\?|$)/.test(r.name)) || performance.now() > 20000".into()),
            script: "({ready: !!document.querySelector('video')})".into(),
            timeout_ms: Some(30000),
            cookies: true,
            headless: Some(true),
            capture_requests: vec![WebViewRequestCapture { url_contains: Some(".m3u8".into()), limit: Some(32), ..Default::default() }],
            method: Default::default(), body: None, headers: Vec::new(), user_agent: None,
            wait_for_selector: None, wait_for_event: None,
            preload_scripts: Vec::new(), capture_events: Vec::new(),
        }
}
impl OceanVeil {
    fn catalog(&self, latest: bool) -> Result<Value> {
        // Public bulk catalog is also the website's initial catalog. The
        // authenticated search endpoint must not be mistaken for an empty list.
        let path = if latest {
            "newest_addition_anime_titles"
        } else {
            "anime_titles"
        };
        let url=format!("{BASE}/api/v1/{path}?limit=5000&is_mature=false&include[]=anime_episodes&include[]=genre&include[]=tags");
        let response = self.client.get(url).send()?.error_for_status()?;
        let data: Value = serde_json::from_str(response.text()?)
            .map_err(|_| Error::new("OceanVeil catalog is not JSON"))?;
        if rows(&data)?.len() >= 5000 {
            return Err(Error::new(
                "OceanVeil catalog exceeded its safety limit; pagination support needs updating",
            ));
        }
        Ok(data)
    }
    fn browse(&self, q: &str, page: u32, latest: bool) -> Result<Paged<CatalogItem>> {
        let data = self.catalog(latest)?;
        let q = q.to_lowercase();
        let items: Vec<_> = rows(&data)?
            .iter()
            .map(|r| title(r, &data))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|i| i.title.to_lowercase().contains(&q))
            .collect();
        let start = page.max(1).saturating_sub(1) as usize * 30;
        let next = items.len() > start + 30;
        Ok(Paged::new(
            items.into_iter().skip(start).take(30).collect(),
            next,
        ))
    }
    fn episode_rows(data: &Value, title_id: &str) -> Result<Vec<VideoEpisode>> {
        let row = rows(data)?
            .iter()
            .find(|r| r["id"].as_str() == Some(title_id))
            .ok_or_else(|| Error::new("This OceanVeil title is not in the public catalog"))?;
        let a = attributes(row)?;
        let refs = row["relationships"]["animeEpisodes"]["data"]
            .as_array()
            .ok_or_else(|| Error::new("OceanVeil episode index is missing"))?;
        let included = data["included"]
            .as_array()
            .ok_or_else(|| Error::new("OceanVeil episodes are missing"))?;
        let mut episodes = Vec::new();
        for reference in refs {
            let row = included
                .iter()
                .find(|r| r["type"] == reference["type"] && r["id"] == reference["id"])
                .ok_or_else(|| Error::new("OceanVeil episode reference is unresolved"))?;
            let eid = id(row)?;
            let e = attributes(row)?;
            let mut episode = VideoEpisode {
                key: eid.into(),
                title: e["name"].as_str().map(str::to_owned),
                description: e["summary"].as_str().map(str::to_owned),
                episode_number: e["displayNumber"]
                    .as_str()
                    .and_then(|s| s.parse().ok())
                    .or_else(|| e["displayNumber"].as_f64().map(|v| v as f32)),
                duration_seconds: e["durationSeconds"].as_f64(),
                language: Some("en".into()),
                url: Some(format!("{BASE}/anime_titles/{title_id}?episode={eid}")),
                ..Default::default()
            };
            episode
                .extra
                .insert("drmRequired".into(), a["isDrmRequired"].clone());
            episode.extra.insert(
                "accessibleInCountry".into(),
                e["isAccessibleInCountry"].clone(),
            );
            if e["isFree"].as_bool() != Some(true) {
                episode.labels.push("Subscription required".into());
            }
            if a["isDrmRequired"].as_bool() == Some(true) {
                episode.labels.push("Official DRM player required".into());
            }
            episodes.push(episode);
        }
        // The website's relationship list is oldest-first. Preserve parts and
        // unnumbered specials instead of relocating them by numeric sorting.
        episodes.reverse();
        Ok(episodes)
    }
    fn streams_from_capture(
        response: &WebViewExtractResponse,
        episode_id: &str,
    ) -> Result<Vec<VideoStream>> {
        let padded = format!("{:09}", numeric(episode_id)?);
        let marker = format!(
            "/anime_episodes/{}/{}/{padded}/v1/video.m3u8",
            &padded[..3],
            &padded[..6]
        );
        let mut streams = Vec::new();
        for r in &response.captured_requests {
            let Ok(url) = Url::parse(&r.url) else {
                continue;
            };
            // Only this episode's complete, non-DRM playlist. A trailer or paid
            // preview is not a working full episode and must never be returned.
            if url.scheme() != "https"
                || url.host_str() != Some("anime-contents-stream.oceanveil.net")
                || url.path() != marker
            {
                continue;
            }
            if streams.iter().any(|s: &VideoStream| s.url == r.url) {
                continue;
            }
            let mut headers: Vec<_> = r
                .headers
                .iter()
                .filter(|(k, _)| {
                    matches!(
                        k.to_lowercase().as_str(),
                        "cookie" | "user-agent" | "referer" | "origin"
                    )
                })
                .cloned()
                .collect();
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("referer"))
            {
                headers.push(("Referer".into(), BASE.into()));
            }
            let cookies: Vec<_> = response
                .cookies
                .iter()
                .filter(|c| {
                    let domain = c.domain.trim_start_matches('.');
                    c.domain.trim_start_matches('.') == "anime-contents-stream.oceanveil.net"
                        && url.host_str().is_some_and(|h| h == domain)
                        && url.path().starts_with(c.path.as_deref().unwrap_or("/"))
                })
                .map(|c| format!("{}={}", c.name, c.value))
                .collect();
            if !cookies.is_empty()
                && !headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("cookie"))
            {
                headers.push(("Cookie".into(), cookies.join("; ")));
            }
            streams.push(VideoStream {
                url: r.url.clone(),
                name: Some("OceanVeil — full episode".into()),
                is_hls: true,
                format: Some("hls".into()),
                requires_proxy: true,
                headers: headers.into_iter().collect(),
                ..Default::default()
            });
        }
        if streams.is_empty() {
            return Err(Error::new("OceanVeil did not authorize a full episode. Sign in through source settings, check your subscription and region, or open the official player. Previews are not returned as full episodes."));
        }
        Ok(streams)
    }
}
impl VideoSource for OceanVeil {
    fn popular(&mut self, p: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", p, false)
    }
    fn latest(&mut self, p: u32) -> Result<Paged<CatalogItem>> {
        self.browse("", p, true)
    }
    fn search(&mut self, q: &str, p: u32, _: &Value) -> Result<Paged<CatalogItem>> {
        self.browse(q, p, false)
    }
    fn details(&mut self, i: CatalogItem) -> Result<CatalogItem> {
        let data = self.catalog(false)?;
        let row = rows(&data)?
            .iter()
            .find(|r| r["id"].as_str() == Some(&i.key))
            .ok_or_else(|| Error::new("OceanVeil title not found in the public catalog"))?;
        title(row, &data)
    }
    fn episodes(&mut self, i: CatalogItem) -> Result<Vec<VideoEpisode>> {
        Self::episode_rows(&self.catalog(false)?, &i.key)
    }
    fn streams(&mut self, i: CatalogItem, e: VideoEpisode) -> Result<Vec<VideoStream>> {
        // Revalidate typed public metadata instead of trusting stale host extras.
        let episodes = Self::episode_rows(&self.catalog(false)?, &i.key)?;
        let current = episodes
            .iter()
            .find(|c| c.key == e.key)
            .ok_or_else(|| Error::new("OceanVeil episode not found"))?;
        if current.extra.get("accessibleInCountry") == Some(&Value::Bool(false)) {
            return Err(Error::new(
                "OceanVeil does not make this episode available in your region.",
            ));
        }
        if current.extra.get("drmRequired") == Some(&Value::Bool(true)) {
            return Err(Error::new("This OceanVeil title requires DRM. Open its official player; native playback is not supported for DRM-protected titles."));
        }
        if current
            .extra
            .get("drmRequired")
            .and_then(Value::as_bool)
            .is_none()
        {
            return Err(Error::new(
                "OceanVeil DRM metadata is missing; playback cannot safely proceed.",
            ));
        }
        let response: WebViewExtractResponse = browser::extract(&player_request(
            current
                .url
                .clone()
                .ok_or_else(|| Error::new("OceanVeil player URL is missing"))?,
        ))?;
        let streams = Self::streams_from_capture(&response, &e.key)?;
        for stream in &streams {
            let mut request = self
                .client
                .get(&stream.url)
                .cookies_for(MEDIA)
                .max_body_bytes(2 * 1024 * 1024);
            for (key, value) in &stream.headers {
                // Guest HTTP uses host-owned same-origin cookies, not raw headers.
                if !key.eq_ignore_ascii_case("cookie") {
                    request = request.header(key, value);
                }
            }
            let playlist = request.send()?;
            if playlist.status() == 401 || playlist.status() == 403 {
                return Err(Error::new("OceanVeil playback authorization expired or was denied. Sign in again and check your subscription and region."));
            }
            if playlist.status() != 200 || !playlist.text()?.trim_start().starts_with("#EXTM3U") {
                return Err(Error::new("OceanVeil did not return an authorized HLS playlist. Open the official player or retry."));
            }
        }
        Ok(streams)
    }
    fn authentication_status(&mut self) -> Result<AuthenticationState> {
        let response:WebViewExtractResponse=browser::extract(&WebViewExtractRequest{
            url:format!("{BASE}/lp"),session:Some(WebViewSession{id:"oceanveil".into(),..Default::default()}),
            script:r#"(async () => {
                const cookie = document.cookie.split('; ').find(c => c.startsWith('auth._token.local='));
                if (!cookie) return {authenticated:false};
                let token = decodeURIComponent(cookie.slice(cookie.indexOf('=')+1));
                try { token = JSON.parse(token); } catch (_) {}
                if (typeof token !== 'string' || !token.startsWith('Bearer ')) return {authenticated:false};
                const response = await fetch('/api/v1/users/me', {headers:{Authorization:token},credentials:'include'});
                return {authenticated:response.ok};
            })()"#.into(),
            timeout_ms:Some(15000),headless:Some(true),wait_until:Some(WebViewWaitUntil::DomReady),
            method:Default::default(),body:None,cookie_url:Some(BASE.into()),headers:Vec::new(),user_agent:None,
            wait_for_script:None,wait_for_selector:None,wait_for_event:None,cookies:true,preload_scripts:Vec::new(),capture_requests:Vec::new(),capture_events:Vec::new()
        })?;
        let authenticated = response
            .value
            .as_ref()
            .or(response.json.as_ref())
            .and_then(|v| v["authenticated"].as_bool())
            .ok_or_else(|| Error::new("OceanVeil login status could not be checked"))?;
        if authenticated {
            Ok(AuthenticationState {
                authenticated: true,
                message: Some(
                    "Signed in. Subscription, region and DRM restrictions still apply.".into(),
                ),
                ..Default::default()
            })
        } else {
            Ok(login_action())
        }
    }
    fn authenticate(&mut self, request: AuthenticationRequest) -> Result<AuthenticationState> {
        if request.interactive {
            Ok(login_action())
        } else {
            self.authentication_status()
        }
    }
    fn logout(&mut self) -> Result<AuthenticationState> {
        // Clearing only the browser profile leaves the host HTTP cookie jar
        // able to restore old login or media grants on the next navigation.
        for origin in [BASE, MEDIA] {
            let mut snapshot = manatan_sdk::cookies::get(origin).map_err(Error::new)?;
            for cookie in &mut snapshot.cookies {
                cookie.value.clear();
                cookie.expires_at = Some(0);
            }
            manatan_sdk::cookies::set(origin, &snapshot.cookies).map_err(Error::new)?;
        }
        let _: browser::WebViewResponse = browser::open(&browser::WebViewRequest {
            url: format!("{BASE}/lp"),
            session: Some(WebViewSession {
                id: "oceanveil".into(),
                clear: true,
                ..Default::default()
            }),
            ..Default::default()
        })?;
        Ok(login_action())
    }
    fn handle_url(&mut self, s: &str) -> Result<Option<UrlResolveResult>> {
        let Ok(url) = Url::parse(s) else {
            return Ok(None);
        };
        if url.scheme() != "https" || url.host_str() != Some("oceanveil.net") {
            return Ok(None);
        }
        let Some(id) = url
            .path()
            .trim_end_matches('/')
            .strip_prefix("/anime_titles/")
        else {
            return Ok(None);
        };
        if numeric(id).is_err() {
            return Ok(None);
        }
        let mut i = CatalogItem::new(id, "");
        i.url = Some(format!("{BASE}/anime_titles/{id}"));
        Ok(Some(UrlResolveResult {
            item: Some(i),
            ..Default::default()
        }))
    }
}
#[cfg(target_arch = "wasm32")]
fn extension() -> manatan_sdk::Extension {
    manatan_sdk::Extension::new().video("oceanveil", OceanVeil::default())
}
#[cfg(target_arch = "wasm32")]
manatan_sdk::export_extension!(extension());
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_cookie_scope_matches_player_origin_and_profile() {
        let request = player_request(format!("{BASE}/anime_titles/1?episode=2"));
        assert_eq!(
            Url::parse(&request.url).unwrap().origin(),
            Url::parse(request.cookie_url.as_deref().unwrap())
                .unwrap()
                .origin()
        );
        assert!(request.cookies);
        assert_eq!(request.session.unwrap().id, "oceanveil");
        assert!(request
            .headers
            .iter()
            .all(|(name, _)| !name.eq_ignore_ascii_case("cookie")));
    }
    #[test]
    fn cover_uses_site_padding_contract() {
        assert!(cover_path("544")
            .unwrap()
            .ends_with("/000/000000/000000544/000000544_vertical_with_logo.jpg_small.webp"));
        assert!(cover_path("../1").is_err());
    }
    #[test]
    fn previews_and_other_episodes_cannot_masquerade_as_full_video() {
        let mut r = WebViewExtractResponse::default();
        for path in [
            "/anime_episodes/000/000002/000002385/v1/preview/video.m3u8",
            "/anime_episodes/000/000002/000002386/v1/video.m3u8",
            "/anime_episodes/000/000002/000002385/v2/2385.m3u8",
        ] {
            r.captured_requests.push(browser::WebViewCapturedRequest {
                url: format!("{MEDIA}{path}"),
                ..Default::default()
            });
        }
        assert!(OceanVeil::streams_from_capture(&r, "2385").is_err());
    }
    #[test]
    fn full_playlist_preserves_only_matching_media_cookies() {
        let r = WebViewExtractResponse {
            captured_requests: vec![browser::WebViewCapturedRequest {
                url: format!("{MEDIA}/anime_episodes/000/000002/000002385/v1/video.m3u8"),
                ..Default::default()
            }],
            cookies: vec![
                browser::WebViewCookie {
                    name: "signed-media".into(),
                    value: "fixture".into(),
                    domain: "anime-contents-stream.oceanveil.net".into(),
                    ..Default::default()
                },
                browser::WebViewCookie {
                    name: "auth".into(),
                    value: "not-for-cdn".into(),
                    domain: "oceanveil.net".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let s = OceanVeil::streams_from_capture(&r, "2385").unwrap();
        assert_eq!(
            s[0].headers
                .iter()
                .find(|(k, _)| k.as_str() == "Cookie")
                .unwrap()
                .1,
            "signed-media=fixture"
        );
    }
    #[test]
    fn unresolved_episode_reference_is_an_error() {
        let v = json!({"data":[{"id":"1","attributes":{},"relationships":{"animeEpisodes":{"data":[{"type":"animeEpisode","id":"2"}]}}}],"included":[]});
        assert!(OceanVeil::episode_rows(&v, "1").is_err());
    }
    #[test]
    fn newest_first_keeps_unnumbered_specials_and_duplicate_numbers() {
        let v = json!({"data":[{"id":"1","attributes":{"isDrmRequired":false},"relationships":{"animeEpisodes":{"data":[{"type":"animeEpisode","id":"2"},{"type":"animeEpisode","id":"3"},{"type":"animeEpisode","id":"4"}]}}}],"included":[{"id":"2","type":"animeEpisode","attributes":{"displayNumber":"1"}},{"id":"3","type":"animeEpisode","attributes":{"displayNumber":"1"}},{"id":"4","type":"animeEpisode","attributes":{"displayNumber":"SP"}}]});
        let episodes = OceanVeil::episode_rows(&v, "1").unwrap();
        assert_eq!(
            episodes.iter().map(|e| e.key.as_str()).collect::<Vec<_>>(),
            vec!["4", "3", "2"]
        );
        assert_eq!(episodes[0].episode_number, None);
        assert_eq!(episodes[1].episode_number, episodes[2].episode_number);
    }
}
