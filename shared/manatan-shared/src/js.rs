//! Small JavaScript extraction helpers used by source readers.
//!
//! This module intentionally does not try to be a JavaScript runtime. It covers
//! common static payload formats used by readers and returns `None` when a page
//! needs real WebView script execution.

use std::collections::HashMap;

pub fn unpack_dean_edwards_script(script: &str) -> Option<String> {
    let args = packed_eval_args(script)?;
    let payload = unescape_js_string(args.first()?)?;
    let radix = args.get(1)?.trim().parse::<u32>().ok()?;
    let count = args.get(2)?.trim().parse::<usize>().ok()?;
    let symbols = split_symbol_table(args.get(3)?)?;
    if radix < 2 || symbols.len() < count {
        return None;
    }

    let mut lookup = HashMap::new();
    for index in (0..count).rev() {
        if let Some(symbol) = symbols.get(index).filter(|value| !value.is_empty()) {
            lookup.insert(encode_radix(index as u32, radix), (*symbol).to_string());
        }
    }

    Some(replace_word_tokens(&payload, &lookup))
}

pub fn extract_dean_edwards_payloads(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = input;
    while let Some(index) = rest.find("eval(function(p,a,c,k,e") {
        rest = &rest[index..];
        if let Some(value) = unpack_dean_edwards_script(rest) {
            out.push(value);
        }
        rest = &rest["eval(".len()..];
    }
    out
}

fn packed_eval_args(script: &str) -> Option<Vec<String>> {
    let start = script.find("eval(function(p,a,c,k,e")?;
    let after_eval = &script[start..];
    if let Some(invoke) = after_eval.find("})(") {
        return parse_call_args(&after_eval[invoke + 3..]);
    }
    let invoke = after_eval.find("}(")?;
    parse_call_args(&after_eval[invoke + 2..])
}

fn parse_call_args(input: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escape = false;
    let mut depth = 0i32;

    for ch in input.chars() {
        if let Some(active) = quote {
            current.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }

        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                current.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                if depth == 0 {
                    args.push(current.trim().to_string());
                    return Some(args);
                }
                depth -= 1;
                current.push(ch);
            }
            ']' | '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    None
}

fn split_symbol_table(input: &str) -> Option<Vec<String>> {
    let value = input.trim();
    let quoted = unescape_js_string(value)?;
    Some(quoted.split('|').map(ToOwned::to_owned).collect())
}

pub fn unescape_js_string(input: &str) -> Option<String> {
    let value = input.trim();
    let quote = value.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let mut chars = value[quote.len_utf8()..].chars();
    let mut out = String::new();
    let mut escape = false;
    while let Some(ch) = chars.next() {
        if escape {
            match ch {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                'x' => {
                    let hex = chars.by_ref().take(2).collect::<String>();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                'u' => {
                    let hex = chars.by_ref().take(4).collect::<String>();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            }
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else if ch == quote {
            return Some(out);
        } else {
            out.push(ch);
        }
    }
    None
}

fn encode_radix(mut value: u32, radix: u32) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % radix) as usize] as char);
        value /= radix;
    }
    out.into_iter().rev().collect()
}

fn replace_word_tokens(input: &str, lookup: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut token = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            flush_token(&mut out, &mut token, lookup);
            out.push(ch);
        }
    }
    flush_token(&mut out, &mut token, lookup);
    out
}

fn flush_token(out: &mut String, token: &mut String, lookup: &HashMap<String, String>) {
    if token.is_empty() {
        return;
    }
    out.push_str(lookup.get(token).unwrap_or(token));
    token.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_js_string() {
        assert_eq!(
            unescape_js_string(r#"'a\n\x62\u0063'"#).as_deref(),
            Some("a\nbc")
        );
    }

    #[test]
    fn unpacks_dean_edwards_payload() {
        let packed =
            r#"eval(function(p,a,c,k,e,d){}('0.1("2")',3,3,'console|log|ok'.split('|'),0,{}))"#;
        assert_eq!(
            unpack_dean_edwards_script(packed).as_deref(),
            Some(r#"console.log("ok")"#)
        );
    }
}
