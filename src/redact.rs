//! Credential redaction for URLs that deckwatch logs, exports or puts into
//! events and error messages.
//!
//! A URL can carry credentials in three places: the userinfo
//! (`scheme://user:password@host`), the query string (`?token=…`,
//! `?private_token=…`, the signature of a presigned URL) and the fragment.
//! [`url`] masks all three and keeps the scheme, host, port and path, so a
//! log line still says where a request went. [`urls_in_text`] does the same
//! for every `scheme://` URL inside free text, such as an error message.
//!
//! Redact before a URL is logged, not before it is exported: with OTLP log
//! export on, every log line also leaves the process.
//!
//! A secret in the path (some webhook URLs are a secret as a whole) can't be
//! told apart from an ordinary path, so don't log such URLs at all.

use std::borrow::Cow;

/// What replaces a redacted part of a URL.
pub const MASK: &str = "***";

/// `raw` with its userinfo, the value of every query parameter and its
/// fragment replaced by [`MASK`]; the scheme, host, port, path and parameter
/// names are kept. `postgres://app:pw@db:5432/deckwatch?sslmode=require`
/// becomes `postgres://***@db:5432/deckwatch?sslmode=***`.
///
/// It doesn't need a valid URL: without a `scheme://`, the text up to the
/// first `/`, `?` or `#` is taken as the authority, so `user:pw@host:5432`
/// is redacted too.
pub fn url(raw: &str) -> String {
    let (scheme, rest) = match raw.find("://") {
        Some(i) => raw.split_at(i + 3),
        None => ("", raw),
    };
    let mut out = String::with_capacity(raw.len());
    out.push_str(scheme);
    redact_after_scheme(rest, &mut out);
    out
}

/// `text` with every `scheme://` URL in it redacted as by [`url`]. A URL
/// ends at whitespace, a quote or a bracket that can't be part of it, and
/// punctuation that ends a sentence (`.`, `,`, `)`, …) stays outside it.
/// Borrows `text` when there is nothing to redact.
pub fn urls_in_text(text: &str) -> Cow<'_, str> {
    if !text.contains("://") {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        // Everything up to the scheme and "://" is kept as it is.
        out.push_str(&rest[..i + 3]);

        let after = &rest[i + 3..];
        let end = after
            .find(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '"' | '\'' | '<' | '>' | '`' | '\\' | '{' | '}' | '|' | '^'
                    )
            })
            .unwrap_or(after.len());
        let token = &after[..end];
        let url = token.trim_end_matches(['.', ',', ';', ':', '!', ')', ']']);
        redact_after_scheme(url, &mut out);
        out.push_str(&token[url.len()..]);
        rest = &after[end..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Append `rest`, the part of a URL after `scheme://`, to `out` with the
/// userinfo, query values and fragment masked.
fn redact_after_scheme(rest: &str, out: &mut String) {
    // The authority ends at the first '/', '?' or '#'; the userinfo is
    // everything before its last '@' (an unescaped '@' in a password
    // included).
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, rest) = rest.split_at(authority_end);
    match authority.rfind('@') {
        Some(at) => {
            out.push_str(MASK);
            out.push_str(&authority[at..]);
        }
        None => out.push_str(authority),
    }

    let path_end = rest.find(['?', '#']).unwrap_or(rest.len());
    let (path, rest) = rest.split_at(path_end);
    out.push_str(path);

    let (query, fragment) = match rest.strip_prefix('?') {
        Some(query) => match query.split_once('#') {
            Some((query, fragment)) => (Some(query), Some(fragment)),
            None => (Some(query), None),
        },
        None => (None, rest.strip_prefix('#')),
    };
    if let Some(query) = query {
        out.push('?');
        for (n, pair) in query.split('&').enumerate() {
            if n > 0 {
                out.push('&');
            }
            match pair.split_once('=') {
                Some((name, "")) => {
                    out.push_str(name);
                    out.push('=');
                }
                Some((name, _)) => {
                    out.push_str(name);
                    out.push('=');
                    out.push_str(MASK);
                }
                None if pair.is_empty() => {}
                // A bare parameter can be a token by itself.
                None => out.push_str(MASK),
            }
        }
    }
    if let Some(fragment) = fragment {
        out.push('#');
        if !fragment.is_empty() {
            out.push_str(MASK);
        }
    }
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
