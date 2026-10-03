//! Small, fresh helpers for text-first WordPress sources. No page scripts execute.
use manatan_common::{absolute_url, normalize_space, selector};
use manatan_sdk::{
    html::{self, ElementRef, Html},
    model::{ImageRequest, ImageRequestContext, NovelText},
    Error, Result,
};
use regex::Regex;

pub fn first_text(doc: &Html, css: &str) -> Result<Option<String>> {
    Ok(doc
        .select(&selector(css)?)
        .next()
        .map(html::text)
        .map(|s| normalize_space(&s))
        .filter(|s| !s.is_empty()))
}
pub fn image(url: String, referer: &str) -> ImageRequest {
    ImageRequest {
        url,
        headers: [("Referer".into(), referer.into())].into(),
        ..ImageRequest::default()
    }
}
pub fn number(title: &str) -> Option<f32> {
    Regex::new(r"(?i)\b(?:chapter|ch\.?|c)\s*(\d+(?:\.\d+)?)")
        .ok()?
        .captures(title)?
        .get(1)?
        .as_str()
        .parse()
        .ok()
}
pub fn rating(tags: &[String]) -> &'static str {
    if tags.iter().any(|t| {
        ["adult", "smut", "hentai"]
            .iter()
            .any(|a| t.eq_ignore_ascii_case(a))
    }) {
        "adult"
    } else {
        "general"
    }
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn render(el: ElementRef<'_>, base: &str, out: &mut String) -> Result<()> {
    let tag = el.value().name();
    if [
        "script", "style", "iframe", "form", "button", "input", "noscript", "svg", "object",
        "embed",
    ]
    .contains(&tag)
        || el.value().classes().any(|c| {
            ["code-block", "adsbygoogle", "entry-share", "sharedaddy"].contains(&c)
                || c.starts_with("ai-")
                || c.starts_with("google-auto-placed")
        })
    {
        return Ok(());
    }
    let allowed = [
        "p",
        "br",
        "hr",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "b",
        "strong",
        "em",
        "i",
        "u",
        "s",
        "sup",
        "sub",
        "span",
        "ul",
        "ol",
        "li",
        "blockquote",
        "table",
        "tbody",
        "tr",
        "td",
        "th",
        "a",
        "img",
        "figure",
        "figcaption",
    ]
    .contains(&tag);
    if allowed {
        out.push('<');
        out.push_str(tag);
        if let Some(id) = el.value().attr("id") {
            out.push_str(&format!(" id=\"{}\"", escape(id)));
        }
        if tag == "a" || tag == "img" {
            let attr = if tag == "a" { "href" } else { "src" };
            let value = if tag == "img" {
                el.value()
                    .attr("data-src")
                    .or_else(|| el.value().attr(attr))
            } else {
                el.value().attr(attr)
            };
            if let Some(value) = value {
                let url = if tag == "a" && value.starts_with('#') {
                    value.to_owned()
                } else {
                    absolute_url(base, value)?
                };
                if url.starts_with("https://")
                    || url.starts_with("http://")
                    || (tag == "a" && url.starts_with('#'))
                {
                    out.push_str(&format!(" {attr}=\"{}\"", escape(&url)));
                }
            }
        }
        if tag == "img" {
            if let Some(alt) = el.value().attr("alt") {
                out.push_str(&format!(" alt=\"{}\"", escape(alt)));
            }
        }
        out.push('>');
    }
    for child in el.children() {
        if let Some(child_el) = ElementRef::wrap(child) {
            render(child_el, base, out)?;
        } else if let Some(text) = child.value().as_text() {
            out.push_str(&escape(text));
        }
    }
    if allowed && !["img", "br", "hr"].contains(&tag) {
        out.push_str(&format!("</{tag}>"));
    }
    Ok(())
}
pub fn chapter_text(doc: &Html, css: &str, base: &str, title: Option<String>) -> Result<NovelText> {
    let container = doc.select(&selector(css)?).next().ok_or_else(|| Error::new("Chapter text is unavailable; open the chapter on the website to check its access requirements"))?;
    let mut body = String::new();
    render(container, base, &mut body)?;
    let cleaned = html::fragment(&body);
    if normalize_space(&html::text(cleaned.root_element())).is_empty() {
        return Err(Error::new(
            "The website returned no chapter text; check whether this chapter is locked",
        ));
    }
    Ok(NovelText {
        html: Some(body),
        title,
        base_url: Some(base.into()),
        image_context: Some(ImageRequestContext {
            headers: [("Referer".into(), base.into())].into(),
            ..ImageRequestContext::default()
        }),
        ..NovelText::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_formatting_footnotes_images_without_ads_or_scripts() {
        let doc = html::document(
            r##"<div class="content"><p>Hello <em>reader</em><sup><a href="#note">1</a></sup></p><div class="code-block"><p>AD</p><script>evil()</script></div><div><p id="note">Note</p><img data-src="/illustration.png" onerror="bad()"></div><script>tracking()</script></div>"##,
        );
        let text = chapter_text(&doc, ".content", "https://example.com/chapter/", None)
            .unwrap()
            .html
            .unwrap();
        assert!(text.contains("<em>reader</em>"));
        assert!(text.contains("https://example.com/illustration.png"));
        assert!(text.contains("<p id=\"note\">Note</p>"));
        assert!(text.contains("href=\"#note\""));
        for bad in ["AD", "script", "onerror", "evil", "tracking"] {
            assert!(!text.contains(bad));
        }
    }
    #[test]
    fn empty_chapter_is_an_error() {
        assert!(chapter_text(
            &html::document("<div><script>ad()</script></div>"),
            "div",
            "https://example.com",
            None
        )
        .is_err());
    }
    #[test]
    fn only_explicit_adult_tags_are_adult() {
        assert_eq!(rating(&["Ecchi".into(), "Yaoi".into()]), "general");
        assert_eq!(rating(&["Smut".into()]), "adult");
        assert_eq!(number("Chapter 5.2: Example"), Some(5.2));
        assert_eq!(number("Extra 1"), None);
    }
}
