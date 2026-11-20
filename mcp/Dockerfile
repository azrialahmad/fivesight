# fivesight-mcp in HTTP mode, for a server other devices reach (e.g. a homelab on a tailnet).
# Build from the repository root:  docker build -f mcp/Dockerfile -t fivesight-mcp .
# Needs FIVESIGHT_MCP_TOKEN (agents) and, for the app's "Sync now", FIVESIGHT_SYNC_TOKEN. See mcp/compose.yaml.
FROM rust:1-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p fivesight-mcp && cp target/release/fivesight-mcp /fivesight-mcp

FROM debian:bookworm-slim
COPY --from=build /fivesight-mcp /usr/local/bin/fivesight-mcp
VOLUME /data
EXPOSE 8787
ENTRYPOINT ["fivesight-mcp"]
CMD ["--http", "0.0.0.0:8787", "--db", "/data/fivesight.db"]
