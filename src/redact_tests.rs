use super::*;

#[test]
fn userinfo_is_masked() {
    assert_eq!(
        url("postgres://deckwatch:s3cret@db.example:5432/deckwatch"),
        "postgres://***@db.example:5432/deckwatch"
    );
    // A user without a password is masked too.
    assert_eq!(
        url("https://token@git.example/org/repo.git"),
        "https://***@git.example/org/repo.git"
    );
    // An unescaped '@' in the password: the userinfo runs to the last '@'.
    assert_eq!(
        url("mysql://app:p@ss@db.example/deckwatch"),
        "mysql://***@db.example/deckwatch"
    );
}

#[test]
fn query_values_and_fragments_are_masked() {
    assert_eq!(
        url("https://git.example/api/packages/org/plugin.wasm?token=abc123&ref=main"),
        "https://git.example/api/packages/org/plugin.wasm?token=***&ref=***"
    );
    assert_eq!(
        url("postgres://db.example/deckwatch?user=app&password=s3cret"),
        "postgres://db.example/deckwatch?user=***&password=***"
    );
    // Presigned object URL: every signature parameter.
    assert_eq!(
        url("https://bucket.s3.example/p.wasm?X-Amz-Credential=AKIAEXAMPLE&X-Amz-Signature=deadbeef"),
        "https://bucket.s3.example/p.wasm?X-Amz-Credential=***&X-Amz-Signature=***"
    );
    // A bare parameter can be a token; empty values stay empty.
    assert_eq!(
        url("https://h.example/x?abc123&a=&"),
        "https://h.example/x?***&a=&"
    );
    assert_eq!(
        url("https://h.example/cb#access_token=abc"),
        "https://h.example/cb#***"
    );
    assert_eq!(
        url("https://h.example/x?t=abc#frag"),
        "https://h.example/x?t=***#***"
    );
}

#[test]
fn urls_without_credentials_are_unchanged() {
    for plain in [
        "https://otlp.example.com:4318/v1/traces",
        "sqlite:///data/deckwatch.db",
        "sqlite::memory:",
        "minio.minio.svc:9000",
        "https://registry.example/@scope/pkg",
        "",
    ] {
        assert_eq!(url(plain), plain);
    }
    // Only the query value goes.
    assert_eq!(
        url("sqlite:///data/deckwatch.db?mode=rwc"),
        "sqlite:///data/deckwatch.db?mode=***"
    );
}

#[test]
fn a_url_without_a_scheme_is_redacted_too() {
    assert_eq!(url("app:s3cret@db.example:5432/x"), "***@db.example:5432/x");
}

#[test]
fn every_url_in_text_is_redacted() {
    assert_eq!(
        urls_in_text("GET https://alice:s3cret@example.com/x?y=1 failed"),
        "GET https://***@example.com/x?y=*** failed"
    );
    // reqwest's error format, and a sentence: the brackets and punctuation stay.
    assert_eq!(
        urls_in_text(
            "error sending request for url (https://git.example/p.wasm?private_token=abc): \
             connection refused"
        ),
        "error sending request for url (https://git.example/p.wasm?private_token=***): \
         connection refused"
    );
    assert_eq!(
        urls_in_text("tried http://u:p@a.example/x, then https://b.example/y?k=v."),
        "tried http://***@a.example/x, then https://b.example/y?k=***."
    );
    // A URL inside JSON ends at the quote.
    assert_eq!(
        urls_in_text(r#"{"url":"https://u:p@h.example/x?t=1","n":2}"#),
        r#"{"url":"https://***@h.example/x?t=***","n":2}"#
    );
    // A URL nested in a query value goes with the value.
    assert_eq!(
        urls_in_text("see https://a.example/r?to=https://u:p@b.example/?t=x now"),
        "see https://a.example/r?to=*** now"
    );
}

#[test]
fn text_without_credentials_is_unchanged() {
    for text in [
        "no urls here",
        "token@host.example and https://example.com",
        "https://example.com/a/b",
        "é https://例え.example/パス é",
    ] {
        let redacted = urls_in_text(text);
        assert_eq!(redacted, text);
    }
    assert!(matches!(urls_in_text("no urls here"), Cow::Borrowed(_)));
}

#[test]
fn multibyte_text_around_urls_is_kept() {
    assert_eq!(
        urls_in_text("é→https://ü:pä@h.example/ü?ä=ö ←"),
        "é→https://***@h.example/ü?ä=*** ←"
    );
}
