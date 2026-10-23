# Contributing

Thanks for helping. A few conventions keep the history easy to read.

## Workflow

1. Branch from `main` as `type/short-topic`, for example `feat/tray-icon` or `fix/echo-dedupe`.
2. Commit with [Conventional Commits](https://www.conventionalcommits.org):
   `type(scope): summary` in the imperative, under about 72 characters. Use the body to say why.
   - Types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`, `style`.
   - Scopes are usually a crate or area: `stt`, `vad`, `audio`, `pipeline`, `store`, `llm`,
     `kb`, `atlassian`, `bridge`, `ask`, `ui`, `extension`.
3. Open a pull request with a short summary and how you tested it. PRs are squash-merged, so the
   PR title becomes the commit on `main` and should follow the same format.

## Before you push

```
cargo fmt --all
cargo test -p fs-store -p fs-kb -p fs-llm -p fs-atlassian -p fs-bridge -p fs-stt -p fs-vad -p fs-audio
cd app && npm run build
cd extension && node --test test/tracker.test.js test/selectors.test.js
```

CI runs the same checks. Crates that need whisper.cpp, CUDA or ONNX Runtime (`fs-pipeline`, the
app, and the `engine`/`silero` features) are built locally with `scripts\build-cuda.bat`.

## Ground rules

- **Read-only Atlassian.** Never add a write command to `fs-atlassian`'s allowlist.
- **No hidden recording.** fivesight must stay visible to the person using it and never try to
  hide from other meeting participants.
- **Untrusted text.** Transcripts, files and tickets go to the model as data inside tags, never as
  instructions. AI output is sanitized before it is rendered.
