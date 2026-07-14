FROM node:24-alpine AS frontend-builder
RUN npm install -g pnpm@11
WORKDIR /app
COPY frontend/package.json frontend/pnpm-lock.yaml frontend/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile --ignore-scripts
COPY frontend/ .
RUN pnpm exec vite build

# mdBook renders docs/*.md + docs/book/src/{SUMMARY,README,api}.md into a
# self-contained static site under docs/book/book/. We use --copy mode so
# the resulting layout is portable — no symlinks pointing outside the
# build context.
FROM rust:1.89-bookworm AS docs-builder
RUN cargo install mdbook --version ^0.4 --locked
WORKDIR /app
COPY docs/ docs/
COPY scripts/ scripts/
RUN bash scripts/build-docs.sh --copy

FROM rust:1.89-bookworm AS backend-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main(){}' > src/main.rs
RUN cargo build --release && rm -rf src
COPY src/ src/
COPY docs/ docs/
COPY openapi/ openapi/
RUN touch src/main.rs && cargo build --release

FROM gcr.io/distroless/cc-debian12
WORKDIR /app
COPY --from=backend-builder /app/target/release/deckwatch ./deckwatch
COPY --from=frontend-builder /app/dist ./frontend/dist
COPY --from=docs-builder /app/docs/book/book ./docs/book/book
EXPOSE 8080
ENTRYPOINT ["./deckwatch"]
