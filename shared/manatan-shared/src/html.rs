pub fn strip_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    html_unescape(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn html_unescape(input: &str) -> String {
    input
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

pub fn attr(input: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{name}={quote}");
        let Some(start) = input.find(&needle).map(|index| index + needle.len()) else {
            continue;
        };
        let rest = &input[start..];
        let end = rest.find(quote)?;
        return Some(html_unescape(&rest[..end]));
    }
    None
}

pub fn attr_after(input: &str, marker: &str, name: &str) -> Option<String> {
    let start = input.find(marker)?;
    attr(&input[start..], name)
}

pub fn text_between(input: &str, start: &str, end: &str) -> Option<String> {
    let start_index = input.find(start)?;
    let after_start = &input[start_index..];
    let content_start = after_start
        .find('>')
        .map(|idx| idx + 1)
        .unwrap_or(start.len());
    let rest = &after_start[content_start..];
    let end_index = rest.find(end)?;
    Some(rest[..end_index].to_string())
}

pub fn class_blocks<'a>(input: &'a str, class_name: &str) -> impl Iterator<Item = &'a str> {
    input.split("<").filter(move |chunk| {
        chunk.contains(&format!("class=\"{class_name}"))
            || chunk.contains(&format!("class='{class_name}"))
            || chunk.contains(&format!(" {class_name} "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_and_unescapes_text() {
        assert_eq!(strip_tags("<p>A &amp; B&nbsp;</p>"), "A & B");
    }
}
