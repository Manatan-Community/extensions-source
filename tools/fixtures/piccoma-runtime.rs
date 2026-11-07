// Production Wasmtime component checks. The fixture host is deliberately
// separate from live HTTP; fixture success is not proof of live reader access.
use manatan_extension::{
    abi::{HttpRequest, HttpResponse},
    parse_archive,
    runner::{ExtensionRunner, HostCall, RunnerError},
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf, process::Command, sync::Arc};

struct Host {
    root: PathBuf,
    live: bool,
}
impl HostCall for Host {
    fn call(&self, operation: &str, payload: &[u8]) -> Result<Vec<u8>, RunnerError> {
        match operation {
            "http.fetch" => {
                let request: HttpRequest = serde_json::from_slice(payload)?;
                assert_eq!(request.method, "GET");
                assert!(request.url.starts_with("https://piccoma.com/"));
                let response = if self.live {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    let mut command = Command::new("curl");
                    command.args([
                        "--silent",
                        "--show-error",
                        "--max-time",
                        "30",
                        "--max-filesize",
                        "4000000",
                        "--write-out",
                        "\n%{http_code}",
                    ]);
                    for (key, value) in &request.headers {
                        command.args(["--header", &format!("{key}: {value}")]);
                    }
                    let output = command
                        .arg("--")
                        .arg(&request.url)
                        .output()
                        .expect("execute curl");
                    assert!(output.status.success(), "live HTTP transfer failed");
                    let body = String::from_utf8(output.stdout).expect("HTML/JSON response");
                    let (text, status) = body.rsplit_once('\n').expect("curl status");
                    HttpResponse {
                        status: status.trim().parse().unwrap(),
                        final_url: request.url,
                        headers: vec![],
                        body_base64: None,
                        text: Some(text.into()),
                    }
                } else {
                    let path = request.url.strip_prefix("https://piccoma.com").unwrap();
                    let fixture = match path {
                        "/web/ranking/K/P/0" => "catalog.html",
                        "/web/product/101" => "details.html",
                        "/web/product/101/episodes?etype=E" => "episodes.html",
                        "/web/product/101/episodes?etype=V" => "volumes.html",
                        "/web/viewer/101/201" | "/web/viewer/101/301" | "/web/viewer/101/302" => {
                            "reader.html"
                        }
                        "/web/viewer/101/203" => "locked.html",
                        other => panic!("unhandled fixture request: {other}"),
                    };
                    let text = if fixture == "locked.html" {
                        "<p>Log in and purchase this chapter.</p>".into()
                    } else {
                        std::fs::read_to_string(
                            self.root
                                .join("manga/ja/piccoma/tests/fixtures")
                                .join(fixture),
                        )
                        .unwrap()
                    };
                    HttpResponse {
                        status: 200,
                        final_url: request.url,
                        headers: vec![],
                        body_base64: None,
                        text: Some(text),
                    }
                };
                Ok(serde_json::to_vec(&json!({"Ok": response}))?)
            }
            "host.log" | "system.sleep" => Ok(serde_json::to_vec(&json!({"Ok":null}))?),
            other => Err(RunnerError::Host(format!(
                "unsupported test-host operation {other}"
            ))),
        }
    }
}
fn call(runner: &ExtensionRunner, operation: &str, mut request: Value) -> Value {
    request["sourceId"] = json!("piccoma");
    runner
        .call_value(operation, request)
        .unwrap_or_else(|error| panic!("{operation}: {error}"))
}
fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("repository root"));
    match args.next().as_deref() {
        Some("fixtures") => run(root, false),
        Some("live") => run(root, true),
        Some("all") => {
            run(root.clone(), false);
            run(root, true);
        }
        _ => panic!("choose fixtures, live, or all"),
    }
}
fn run(root: PathBuf, live: bool) {
    let bytes = std::fs::read(root.join("dist/packages/manga/ja/piccoma.manatan2")).unwrap();
    let archive = parse_archive(&bytes).expect("signed package must validate");
    assert_eq!(archive.manifest.id, "piccoma");
    let runner = ExtensionRunner::with_host(
        archive,
        Arc::new(Host {
            root: root.clone(),
            live,
        }),
    );
    assert_eq!(
        call(&runner, "filters", json!({}))[0]["options"]
            .as_array()
            .unwrap()
            .len(),
        23
    );
    assert_eq!(
        call(&runner, "preferences", json!({}))[0]["key"],
        "hide_locked"
    );
    let ranking = call(&runner, "manga.list", json!({"listing":"popular","page":1}));
    let entries = ranking["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    assert_eq!(
        entries
            .iter()
            .map(|e| e["key"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        entries.len()
    );
    assert!(
        call(&runner, "manga.list", json!({"listing":"popular","page":2}))["entries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    if live {
        let latest = call(&runner, "manga.list", json!({"listing":"latest","page":1}));
        assert!(!latest["entries"].as_array().unwrap().is_empty());
        let searched = call(&runner, "manga.search", json!({"query":"異世界","page":1}));
        assert!(!searched["entries"].as_array().unwrap().is_empty());
        if searched["hasNextPage"] == true {
            let page2 = call(&runner, "manga.search", json!({"query":"異世界","page":2}));
            assert!(!page2["entries"].as_array().unwrap().is_empty());
            assert_ne!(searched["entries"][0]["key"], page2["entries"][0]["key"]);
        }
        let cover = entries[0]["cover"]["url"].as_str().unwrap();
        assert!(cover.starts_with("https://piccoma.kakaocdn.net/"));
        let response = Command::new("curl")
            .args([
                "--fail",
                "--silent",
                "--show-error",
                "--max-time",
                "30",
                "--referer",
                "https://piccoma.com/web/",
                "--",
                cover,
            ])
            .output()
            .unwrap();
        assert!(response.status.success() && response.stdout.len() > 100);
        std::fs::write(
            root.join("target/piccoma-live-cover.image"),
            response.stdout,
        )
        .unwrap();
        let status = call(&runner, "auth.status", json!({}));
        assert!(status["authenticated"].is_boolean());
        let details = runner.call_value(
            "manga.details",
            json!({"sourceId":"piccoma","manga":entries[0]}),
        );
        match details {
            Ok(_) => println!("Live product access is available; reader verification still requires an accessible chapter."),
            Err(error) => { assert!(error.to_string().contains("Japan")); println!("Live product access BLOCKED: Piccoma requires a network in Japan."); }
        }
        println!("Live production-runner checks PASS: {} ranking entries, latest, search pagination, login-state parsing, cover transfer. Reader access NOT live-verified.", entries.len());
    } else {
        let item = call(&runner, "manga.details", json!({"manga":entries[0]}));
        assert_eq!(item["title"], "テスト作品");
        let chapters = call(&runner, "manga.chapters", json!({"manga":item}));
        let chapters = chapters.as_array().unwrap();
        assert_eq!(chapters.len(), 7);
        assert_eq!(chapters[0]["chapterNumber"], 4.);
        assert_eq!(chapters.last().unwrap()["volumeNumber"], 1.);
        for id in ["201", "302", "301"] {
            let chapter = chapters.iter().find(|c| c["key"] == id).unwrap();
            let pages = call(
                &runner,
                "manga.pages",
                json!({"manga":item,"chapter":chapter}),
            );
            assert_eq!(pages.as_array().unwrap().len(), 2);
            assert!(pages[0]["extra"]["piccomaSeed"].is_string());
        }
        let locked = chapters.iter().find(|c| c["key"] == "203").unwrap();
        assert!(runner
            .call_value(
                "manga.pages",
                json!({"sourceId":"piccoma","manga":item,"chapter":locked})
            )
            .unwrap_err()
            .to_string()
            .contains("purchase"));
        let page = json!({"content":{"url":{"url":"https://piccoma.kakaocdn.net/fixture.png"}},"extra":{"piccomaSeed":"fixture"}});
        let output = runner
            .process_page_bytes(
                "piccoma",
                json!({"manga":item,"chapter":chapters[0],"page":page}),
                std::fs::read(root.join("manga/ja/piccoma/assets/icon.png")).unwrap(),
                Some("image/png".into()),
            )
            .unwrap()
            .expect("image transform");
        assert_eq!(&output.bytes[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(output.mime_type.as_deref(), Some("image/png"));
        println!("Fixture production-runner checks PASS: catalog, metadata, newest-first chapters, free/preview page lists, locked-chapter failure, binary tile transform.");
    }
}
