use super::*;
use crate::test_tracing::capture;

#[tokio::test]
async fn the_database_password_is_never_logged() {
    let ((), captured) = capture(async {
        log_connecting_to_database(
            "postgres://deckwatch:userinfo-pw@db.example:5432/deckwatch?password=query-pw",
        );
    })
    .await;

    let events = captured.events_on("deckwatch");
    let event = events
        .iter()
        .find(|e| e.message == "connecting to database")
        .expect("the connecting line is logged");
    assert_eq!(event.fields["backend"], "postgres");
    assert_eq!(
        event.fields["url"],
        "postgres://***@db.example:5432/deckwatch?password=***"
    );
    let everything = captured.haystack();
    assert!(!everything.contains("userinfo-pw"), "{everything}");
    assert!(!everything.contains("query-pw"), "{everything}");
}
