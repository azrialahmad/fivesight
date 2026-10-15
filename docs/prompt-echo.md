# Prompt echoes

Whisper's initial prompt is the meeting's glossary (title, word list, people, terms) followed by
one example sentence, `BILINGUAL_PROMPT` in `crates/fs-stt/src/lang.rs`, that sets the mixed
Indonesian/English style. On short or unclear audio Whisper sometimes just says part of the prompt
back. `echoes_prompt` (`crates/fs-stt/src/hints.rs`) drops such a line, and if someone really
spoke, their words go with it.

## What the first real meeting showed

An hour-long Google Meet call on 2026-10-15 dropped 35 finals as "repeats the prompt". 34 were
the old example sentence's English clause ("Let's review the sprint." or "... sprint backlog.")
and one was its opener ("Oke, kita mulai meeting-nya."). The log didn't say how long each line
was or whose it was, and no audio is kept, so there is no exact count of how many hid real speech.
What the log does show:

- Every one got through the energy gate, the VAD (at least 0.5 s of speech) and the
  no-speech/log-probability filter, so Whisper was fairly sure someone was talking.
- They come in bursts during busy talk (four within 7 s at 08:13:47, pairs 0.6 s apart).
- Played through `fivesight-cli replay`, real speech gets dropped this way: TTS saying "Deploy to
  staging" came out as the prompt's "Deploy ke staging" and was dropped. A 1 s filler word came out
  as "Let's review the sprint."

So most of them were short real utterances: backchannels and fillers, sometimes a real phrase.

## What changed

1. **A less sayable example sentence.** One Indonesian sentence with the English words inside it
   (`PR-nya`, `di-review`, `deploy ke staging`). It has no stand-alone English clause and no
   opener people say on their own. Whisper repeats whole clauses, so this removes the one
   that came back 34 times.
2. **A second try before dropping.** When a final only repeats the prompt, `fs_pipeline::listen_again`
   transcribes the same audio again with the glossary but without the example sentence
   (`hints::retry_prompt`). Whisper can't say back what isn't in its prompt, so if the second try
   hears something other than its own smaller prompt, that line is kept. This costs one extra
   decode per echo, about 35 an hour.
3. **Better logs.** The drop reason now gives the source, length and language
   (`repeats the prompt: Loopback, 1.2 s, en`), and every saved line logs
   `prompt echo "..." heard again without it: ...`.

## Checking it

Against a recording (16 kHz mono WAV), from the repo root where `models/` is:

```
scripts\build-cuda.bat build --release -p fivesight-cli --features cuda
target\release\fivesight-cli replay meeting.wav auto
target\release\fivesight-cli replay meeting.wav auto "<old prompt to compare>"
```

`replay` splits the WAV with the same VAD settings as the other side's audio and transcribes each
line. Lines marked `xx` repeated the prompt and are dropped. Lines marked `<>` repeated it and were
saved by the second try. Pass the meeting's real prompt (the log's last `whisper prompt:` line) to
get its glossary too. `scripts\make-synthetic-wav.ps1` makes an English fixture. It shows that
ordinary speech passes untouched, but it rarely triggers echoes. Short, quiet, muffled clips
(fillers like "hmm", "iya iya" mixed under noise) are closer to what happens in a real meeting.

In the app, after a meeting:

```
findstr /c:"repeats the prompt" /c:"heard again without it" %LOCALAPPDATA%\dev.fivesight.app\logs\fivesight.log
```

The `heard again` lines are speech that the old code would have lost. The `repeats the prompt`
lines that are left are drops where the second try heard nothing usable either.

If cargo fails in a deep worktree with `nvcc fatal : Could not open output file`, the path is too
long for nvcc. Set `CARGO_TARGET_DIR` to a short path first.
