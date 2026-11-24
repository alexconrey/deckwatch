//! The deckwatch server binary. All the code lives in the library
//! (`src/lib.rs`).

#[tokio::main]
async fn main() {
    deckwatch::run().await;
}
