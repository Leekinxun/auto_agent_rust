FROM node:20-bookworm-slim AS frontend-builder

WORKDIR /app/frontend

COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci

COPY frontend ./
RUN npm run build


FROM rust:1.94-bookworm AS backend-builder

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release


FROM debian:bookworm-slim AS runtime

ENV SERVER_HOST=0.0.0.0 \
    SERVER_PORT=18000

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=backend-builder /app/target/release/auto-claude-code-rs ./auto-claude-code-rs
COPY config ./config
COPY skills ./skills
COPY --from=frontend-builder /app/static/frontend ./static/frontend

RUN mkdir -p /app/uploads /app/outputs /app/.user_memories /app/.tasks /app/.worktrees /app/.sessions /app/.transcripts

EXPOSE 18000

CMD ["./auto-claude-code-rs"]
