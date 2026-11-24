# Keep the Node major in step with frontend/.nvmrc, which CI builds with.
# The frontend and docs stages produce platform-independent files (static
# assets and HTML), so they run on the build machine's own platform: a release
# build for another architecture doesn't have to emulate them.
FROM --platform=$BUILDPLATFORM node:26-alpine AS frontend-builder
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
#
# mdBook comes from a pinned release binary with a verified checksum, instead
# of being compiled with `cargo install` on every cache miss. CI's Docs Build
# job reads MDBOOK_VERSION and MDBOOK_SHA256_X86_64 from these ARGs, so bump
# them here and both stay in step.
FROM --platform=$BUILDPLATFORM rust:1.99-bookworm AS docs-builder
ARG BUILDARCH
ARG MDBOOK_VERSION=v0.4.52
ARG MDBOOK_SHA256_X86_64=c0b903f01dd8f4edc644372ad2b80b1fdddd12552d37b6a098657cbd8eddd768
ARG MDBOOK_SHA256_AARCH64=7273dda980915a1e2f114d63d432aa6284551e37f0358e3ce7653d1e49e6fa3f
RUN case "$BUILDARCH" in \
      amd64) target=x86_64-unknown-linux-gnu; sha256="$MDBOOK_SHA256_X86_64" ;; \
      arm64) target=aarch64-unknown-linux-musl; sha256="$MDBOOK_SHA256_AARCH64" ;; \
      *) echo "no pinned mdBook binary for build architecture $BUILDARCH" >&2; exit 1 ;; \
    esac && \
    curl --fail --silent --show-error --location --output /tmp/mdbook.tar.gz \
      "https://github.com/rust-lang/mdBook/releases/download/${MDBOOK_VERSION}/mdbook-${MDBOOK_VERSION}-${target}.tar.gz" && \
    echo "${sha256}  /tmp/mdbook.tar.gz" | sha256sum --check --strict && \
    tar -xzf /tmp/mdbook.tar.gz -C /usr/local/bin mdbook && \
    rm /tmp/mdbook.tar.gz && \
    mdbook --version
WORKDIR /app
COPY docs/ docs/
COPY scripts/ scripts/
RUN bash scripts/build-docs.sh --copy

FROM rust:1.99-bookworm AS backend-builder
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev && \
    rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
# Dependency-only build: stub binary and library so this layer caches.
RUN mkdir src && echo 'fn main(){}' > src/main.rs && touch src/lib.rs
RUN cargo build --release --locked && rm -rf src
COPY src/ src/
COPY docs/ docs/
COPY openapi/ openapi/
RUN touch src/main.rs src/lib.rs && cargo build --release --locked

FROM gcr.io/distroless/cc-debian12
WORKDIR /app
COPY --from=backend-builder /app/target/release/deckwatch ./deckwatch
COPY --from=frontend-builder /app/dist ./frontend/dist
COPY --from=docs-builder /app/docs/book/book ./docs/book/book
EXPOSE 8080
ENTRYPOINT ["./deckwatch"]
