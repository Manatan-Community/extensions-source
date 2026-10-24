use manatan_extension::{
    parse_archive,
    runner::{ExtensionRunner, HostCall, RunnerError},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const SITE: &str = "https://www.anime-jl.net";
const ITEM: &str = "/anime/512/example";
const CATALOG: &str = r#"<ul class="ListAnimes"><li><article class="Anime"><a href="/anime/512/example"><h3 class="Title">Example</h3></a></article></li></ul>"#;
const DETAILS: &str = r#"<div class="Ficha fchlt"><div class="Container"><h1 class="Title">Example</h1></div></div><script>var episodes = [[1,"episodio-1","",""]];</script>"#;
const EPISODE: &str = r#"video[0] = '<iframe src="https://ok.ru/videoembed/123"></iframe>';"#;

#[derive(Default)]
struct ContractHost {
    requests: Mutex<Vec<Value>>,
}

impl HostCall for ContractHost {
    fn call(&self, operation: &str, payload: &[u8]) -> Result<Vec<u8>, RunnerError> {
        if operation != "http.fetch" {
            return Err(RunnerError::Host(format!(
                "unexpected capability: {operation}"
            )));
        }
        let request: Value = serde_json::from_slice(payload)?;
        let url = request["url"].as_str().unwrap();
        self.requests.lock().unwrap().push(request.clone());
        let site_request = url.starts_with(&format!("{SITE}/"));
        // Mirrors the app host's recovery prerequisite. Even a package with
        // WebView/cookie permissions cannot recover a challenge without this
        // explicit per-request cookie scope.
        if site_request {
            assert_eq!(
                request["cookieUrl"].as_str(),
                Some(url),
                "site request must opt into the host cookie/challenge boundary"
            );
        } else {
            assert!(
                request["cookieUrl"].is_null(),
                "site cookie scope leaked into a player request"
            );
            assert_eq!(url, "https://ok.ru/videoembed/123");
        }
        let (status, body) = if !site_request {
            (404, "removed")
        } else if url.ends_with("/episodio-1") {
            (200, EPISODE)
        } else if url.contains("/animes?") {
            (200, CATALOG)
        } else {
            (200, DETAILS)
        };
        Ok(serde_json::to_vec(&json!({"Ok": {
            "status": status, "finalUrl": url, "text": body,
            "headers": [["Content-Type", "text/html"]]
        }}))?)
    }
}

fn main() {
    let archive =
        parse_archive(&std::fs::read(std::env::args().nth(1).expect("package path")).unwrap())
            .unwrap();
    let host = Arc::new(ContractHost::default());
    let runner = ExtensionRunner::with_host(archive, host.clone());
    let item = json!({"key": ITEM, "title": "Example"});
    for (operation, mut request) in [
        ("video.list", json!({"listing": "popular", "page": 1})),
        ("video.list", json!({"listing": "latest", "page": 1})),
        ("video.search", json!({"query": "Example", "page": 1})),
        ("video.details", json!({"item": item})),
        ("video.episodes", json!({"item": item})),
    ] {
        request["sourceId"] = json!("animejl");
        runner
            .call_value(operation, request)
            .expect("guest operation should succeed with scoped site request");
    }
    let result = runner.call_value(
        "video.streams",
        json!({
            "sourceId": "animejl", "item": item,
            "episode": {"key": format!("{ITEM}/episodio-1"), "episodeNumber": 1}
        }),
    );
    assert!(
        result.is_err(),
        "removed player should remain a visible failure"
    );
    let requests = host.requests.lock().unwrap();
    assert_eq!(
        requests.len(),
        7,
        "all site operations and the external player must be checked"
    );
    assert_eq!(
        requests.last().unwrap()["url"],
        "https://ok.ru/videoembed/123"
    );
    println!(
        "AnimeJL HTTP contract passed: 6 scoped site requests; external player remains separate"
    );
}
