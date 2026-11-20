"""Compare fivesight's Whisper with other speech models on the same Indonesian/English clips.

Every model hears the same clips (one utterance each, like the app's pipeline) and gets the same
meeting hints (title, glossary, people), so the report compares like with like.

Get audio, one of:
  uv run compare.py fleurs --n 40                    # public Indonesian clips with correct text
  uv run compare.py record --name standup            # your PC's audio + mic until Ctrl+C
  uv run compare.py prepare some-call.wav --name x   # any recording you already have
Then:
  uv run compare.py run sets/fleurs-id
  uv run compare.py run sets/standup --glossary "Goldplus, MamiAds" --people "Nuki Pratama, Fajar"

The report lands in <set>/report.md. Adding a model: write a runner like `run_qwen` and list it
in MODELS.
"""

from __future__ import annotations

import argparse
import csv
import json
import re
import subprocess
import sys
import tarfile
import time
from pathlib import Path

import numpy as np
import soundfile as sf
import soxr

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SETS = HERE / "sets"
SR = 16_000
WHISPER_BIN = ROOT / "target" / "release" / "examples" / "transcribe_chunks.exe"
WHISPER_MODEL = ROOT / "models" / "ggml-large-v3-turbo.bin"
# The app's example sentence for mixed Indonesian/English meetings (fs_stt::lang::BILINGUAL_PROMPT).
BASE_SENTENCE = "Jadi PR-nya udah di-review, tinggal deploy ke staging terus update ticket-nya."


# ---------- audio ----------


def load_16k(path: Path) -> np.ndarray:
    data, sr = sf.read(str(path), dtype="float32", always_2d=True)
    mono = data.mean(axis=1)
    if sr != SR:
        mono = soxr.resample(mono, sr, SR)
    return mono.astype(np.float32)


def split_utterances(x: np.ndarray, max_s: float = 25.0, min_s: float = 0.6) -> list[tuple[float, np.ndarray]]:
    """Cut a long recording into utterances at pauses, like the app's voice detector roughly does.
    Returns (start in seconds, audio) pairs."""
    hop = 0.03
    frame = int(hop * SR)
    n = len(x) // frame
    if n == 0:
        return []
    rms = np.sqrt((x[: n * frame].reshape(n, frame) ** 2).mean(axis=1) + 1e-12)
    threshold = max(np.percentile(rms, 20) * 3, 1e-4)
    voiced = rms > threshold
    max_gap = int(0.4 / hop)  # pauses shorter than this stay inside one utterance

    spans, start, gap = [], None, 0
    for i, v in enumerate(voiced):
        if v:
            start = i if start is None else start
            gap = 0
        elif start is not None:
            gap += 1
            if gap > max_gap:
                spans.append((start, i - gap + 1))
                start, gap = None, 0
    if start is not None:
        spans.append((start, n))

    # Whisper hears at most 30 s at a time: split long stretches at their quietest moment.
    max_f = int(max_s / hop)
    cut_spans = []
    for s, e in spans:
        while e - s > max_f:
            lo = s + max_f * 2 // 3
            cut = lo + int(np.argmin(rms[lo : s + max_f]))
            cut_spans.append((s, cut))
            s = cut
        cut_spans.append((s, e))

    pad = int(0.15 / hop)
    return [
        (max(0, s - pad) * hop, x[max(0, (s - pad) * frame) : min(len(x), (e + pad) * frame)])
        for s, e in cut_spans
        if (e - s) * hop >= min_s
    ]


def write_set(name: str, clips: list[tuple], source: str, ref: str | None = None) -> Path:
    """`clips`: (audio, correct text or None, extra fields such as track and start time)."""
    out = SETS / name
    if (out / "clips").exists():
        for old in (out / "clips").glob("*.wav"):
            old.unlink()
    (out / "clips").mkdir(parents=True, exist_ok=True)
    with open(out / "manifest.jsonl", "w", encoding="utf-8") as m:
        for i, (audio, clip_ref, *extra) in enumerate(clips):
            wav = out / "clips" / f"{i:04d}.wav"
            sf.write(str(wav), audio, SR, subtype="PCM_16")
            row = {"wav": f"clips/{wav.name}", "ref": clip_ref, "source": source, **(extra[0] if extra else {})}
            m.write(json.dumps(row) + "\n")
    if ref:
        (out / "ref.txt").write_text(ref, encoding="utf-8")
    secs = sum(len(c[0]) for c in clips) / SR
    print(f"wrote {len(clips)} clips ({secs / 60:.1f} min) to {out}")
    return out


# ---------- getting audio ----------


def cmd_fleurs(args):
    """Indonesian clips from Google's FLEURS test set, which come with correct transcripts."""
    from huggingface_hub import hf_hub_download

    tsv = hf_hub_download("google/fleurs", "data/id_id/test.tsv", repo_type="dataset")
    print("downloading FLEURS Indonesian test audio (a few hundred MB, once)...")
    tar = hf_hub_download("google/fleurs", "data/id_id/audio/test.tar.gz", repo_type="dataset")

    rows = {}
    with open(tsv, encoding="utf-8") as f:
        for r in csv.reader(f, delimiter="\t", quoting=csv.QUOTE_NONE):
            rows.setdefault(r[1], r[2])  # file name -> transcript as spoken (with punctuation)
    names = sorted(rows)
    pick = set(names[:: max(1, len(names) // args.n)][: args.n])

    clips = []
    with tarfile.open(tar) as t:
        for member in t:
            base = Path(member.name).name
            if base in pick:
                data, sr = sf.read(t.extractfile(member), dtype="float32")
                if sr != SR:
                    data = soxr.resample(data, sr, SR)
                clips.append((data.astype(np.float32), rows[base]))
    write_set(args.name, clips, "fleurs id_id test")


def cmd_record(args):
    """Record what your PC plays (the other people) and your mic, into two files, until Ctrl+C."""
    import soundcard as sc

    speaker = sc.default_speaker()
    others = sc.get_microphone(id=str(speaker.name), include_loopback=True)
    me = sc.default_microphone()
    import threading
    import warnings

    rec_sr, block = 48_000, 4_800
    print(f"recording '{speaker.name}' (others) and '{me.name}' (you). Ctrl+C to stop.")
    print("Only record calls where everyone is fine with it.")
    bufs = {"others": [], "me": []}
    gaps = {"others": 0, "me": 0}
    stop = threading.Event()

    # One thread per device, so neither waits on the other and overflows its buffer.
    def capture(who, device):
        with warnings.catch_warnings(record=True) as caught:
            warnings.simplefilter("always")
            with device.recorder(samplerate=rec_sr, channels=1, blocksize=block) as rec:
                while not stop.is_set():
                    bufs[who].append(rec.record(numframes=block)[:, 0].copy())
            gaps[who] = sum("discontinuity" in str(w.message) for w in caught)

    threads = [threading.Thread(target=capture, args=a, daemon=True) for a in (("others", others), ("me", me))]
    for t in threads:
        t.start()
    start = time.time()
    try:
        while args.minutes is None or time.time() - start < args.minutes * 60:
            time.sleep(0.2)
    except KeyboardInterrupt:
        pass
    stop.set()
    for t in threads:
        t.join(timeout=2)
    for who, n in gaps.items():
        if n:
            # Windows reports a gap on the PC-audio side whenever nothing is playing; that's harmless.
            print(f"note: {n} gap(s) reported on '{who}'")
    out = HERE / "recordings"
    out.mkdir(exist_ok=True)
    for who, parts in bufs.items():
        if not parts:
            continue
        audio = soxr.resample(np.concatenate(parts), rec_sr, SR).astype(np.float32)
        path = out / f"{args.name}-{who}.wav"
        sf.write(str(path), audio, SR, subtype="PCM_16")
        print(f"saved {path} ({len(audio) / SR / 60:.1f} min)")
    print(f"next: uv run compare.py prepare recordings/{args.name}-others.wav --name {args.name}")


SILERO = ROOT / "models" / "silero_vad.onnx"


def has_speech(audio: np.ndarray, sess) -> bool:
    """The app's rule (fs-vad defaults): Silero at least 0.5 sure on at least 500 ms of frames."""
    frame, ctx = 512, 64
    state = np.zeros((2, 1, 128), dtype=np.float32)
    context = np.zeros(ctx, dtype=np.float32)
    sr = np.array(SR, dtype=np.int64)
    speech = 0
    for i in range(0, len(audio) - frame + 1, frame):
        f = audio[i : i + frame].astype(np.float32)
        out, state = sess.run(None, {"input": np.concatenate([context, f])[None, :], "state": state, "sr": sr})
        context = f[-ctx:]
        if out[0][0] >= 0.5:
            speech += 1
            if speech * frame / SR >= 0.5:
                return True
    return False


def read_answers(path: Path) -> tuple[dict[str, str], set[str]]:
    """Correct text typed into a draft file, by clip wav, and the clips it marked silent."""
    answers, silent, clip = {}, set(), None
    if not path.exists():
        return answers, silent
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("# silent:"):
            silent |= {f"clips/{n}.wav" for n in line.split(":", 1)[1].split()}
            continue
        m = re.match(r"\[(\d+)\]", line)
        if m:
            clip = f"clips/{m.group(1)}.wav"
        elif line.startswith("=") and clip:
            answers[clip] = line[1:].strip()
            clip = None
    return answers, silent


def cmd_draft(args):
    """Write a file for typing the correct text of a stretch, with every model's version to start from.
    Clips the voice detector finds no speech in are marked silent and left out, unless already answered."""
    import onnxruntime as ort

    set_dir = SETS / args.set
    items = [i for i in read_set(set_dir) if args.from_min * 60 <= i.get("start_s", 0) < args.to_min * 60]
    out = set_dir / (args.out or f"correct-{args.from_min:g}-{args.to_min:g}.txt")
    # Carry over what was typed in an earlier draft of this stretch.
    keep_from = set_dir / args.keep if args.keep else out
    answers, _ = read_answers(keep_from)
    typed = {k: v for k, v in answers.items() if v}
    sess = ort.InferenceSession(str(SILERO), providers=["CPUExecutionProvider"])
    silent = []
    for it in items:
        if it["wav"] not in typed and not has_speech(load_16k(it["path"]), sess):
            silent.append(it)
    silent_wavs = {i["wav"] for i in silent}
    versions = {}
    for f in sorted(set_dir.glob("results-*.jsonl")):
        name = f.stem.removeprefix("results-")
        versions[name] = {json.loads(l)["wav"]: json.loads(l)["text"] for l in f.read_text(encoding="utf-8").splitlines() if l}
    shown = [i for i in items if i["wav"] not in silent_wavs]
    by_track = {}
    for i in silent:
        t = i.get("track", "").rsplit("-", 1)[-1]
        by_track[t] = by_track.get(t, 0) + 1
    lines = [
        "# Type what was actually said after '=' for each clip. Leave '=' empty if nothing was said.",
        "# The lines above it are the models' guesses; neither is trusted. Play the clip to check:",
        f"#   {set_dir / 'clips'}",
        f"# When done: uv run compare.py import {args.set} {out.name}",
        "# The voice detector heard no speech in these clips, so they count as silent (move one",
        "# into the list below as a [number] block if you hear someone in it):",
        "# silent: " + " ".join(Path(i["wav"]).stem for i in silent),
        "",
    ]
    for it in shown:
        s = it.get("start_s", 0)
        lines.append(f"[{Path(it['wav']).stem}] {int(s // 60)}:{int(s % 60):02d} {it.get('track', '').rsplit('-', 1)[-1]}")
        for name, v in versions.items():
            t = v.get(it["wav"], "")
            lines.append(f"  {name}: {'(echoed the hints)' if echoes(t) else t}")
        lines += [f"= {typed.get(it['wav'], it.get('ref') or '')}".rstrip() + " ", ""]
    out.write_text("\n".join(lines), encoding="utf-8")
    kept = sum(1 for i in shown if i["wav"] in typed)
    print(f"{len(shown)} clips to check ({kept} already answered), {len(silent)} marked silent {by_track} -> {out}")


def cmd_import(args):
    """Read the correct text typed into a draft file into the set's manifest."""
    set_dir = SETS / args.set
    refs, silent = read_answers(set_dir / args.file)
    for w in silent:
        refs.setdefault(w, "")
    rows = [json.loads(l) for l in (set_dir / "manifest.jsonl").read_text(encoding="utf-8").splitlines() if l]
    for r in rows:
        if r["wav"] in refs:
            r["ref"] = refs[r["wav"]]
    (set_dir / "manifest.jsonl").write_text("\n".join(json.dumps(r) for r in rows) + "\n", encoding="utf-8")
    said = sum(1 for v in refs.values() if v)
    print(f"read {len(refs)} clips ({said} with speech, {len(refs) - said} silent)")
    times = [r["start_s"] for r in rows if r["wav"] in refs and "start_s" in r]
    if times:
        lo, hi = int(min(times) // 60), int(max(times) // 60) + 1
        print(f"score it: uv run compare.py run {args.set} --report-only --from-min {lo} --to-min {hi} <same hints>")


def cmd_prepare(args):
    """Cut recordings into utterances and save them as a set."""
    clips = []
    for p in args.audio:
        track = Path(p).stem
        clips += [(c, None, {"track": track, "start_s": round(t, 2)}) for t, c in split_utterances(load_16k(Path(p)))]
    # In meeting order, so a stretch of clips is a stretch of the meeting.
    clips.sort(key=lambda c: c[2]["start_s"])
    ref = Path(args.ref).read_text(encoding="utf-8") if args.ref else None
    write_set(args.name, clips, ", ".join(args.audio), ref)


# ---------- models ----------


def read_set(set_dir: Path) -> list[dict]:
    items = [json.loads(l) for l in (set_dir / "manifest.jsonl").read_text(encoding="utf-8").splitlines() if l]
    for it in items:
        it["path"] = (set_dir / it["wav"]).resolve()
    return items


def run_whisper(items: list[dict], hints: dict, cpu: bool) -> dict:
    """fivesight's own engine (whisper.cpp, large-v3-turbo), picking id/en per clip like the app."""
    if not WHISPER_BIN.exists():
        sys.exit(f"build it first: scripts\\build-cuda.bat build -p fs-stt --example transcribe_chunks --features cuda --release")
    work = HERE / ".work"
    work.mkdir(exist_ok=True)
    (work / "list.txt").write_text("\n".join(str(i["path"]) for i in items), encoding="utf-8")
    prompt = " ".join(p for p in [hints["context"], BASE_SENTENCE] if p)
    (work / "prompt.txt").write_text(prompt, encoding="utf-8")
    cmd = [str(WHISPER_BIN), str(WHISPER_MODEL), str(work / "list.txt"), str(work / "prompt.txt")]
    if cpu:
        cmd.append("--cpu")
    done = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8")
    if done.returncode != 0:
        sys.exit(f"whisper runner failed:\n{done.stderr[-2000:]}")
    by_file = {}
    for line in done.stdout.splitlines():
        r = json.loads(line)
        by_file[str(Path(r["file"]).resolve())] = r
    return {str(i["path"]): {"text": by_file.get(str(i["path"]), {}).get("text", ""),
                             "ms": by_file.get(str(i["path"]), {}).get("ms", 0)} for i in items}


def run_qwen(items: list[dict], hints: dict, cpu: bool, size: str = "1.7B") -> dict:
    """Qwen3-ASR, given the meeting hints as context (it treats them as vocabulary, not instructions)."""
    import inspect

    import torch
    from qwen_asr import Qwen3ASRModel

    device = "cpu" if cpu or not torch.cuda.is_available() else "cuda:0"
    model = Qwen3ASRModel.from_pretrained(
        f"Qwen/Qwen3-ASR-{size}",
        dtype=torch.float32 if device == "cpu" else torch.bfloat16,
        device_map=device,
        max_inference_batch_size=1,
        max_new_tokens=448,
    )
    takes_context = "context" in inspect.signature(model.transcribe).parameters
    if hints["context"] and not takes_context:
        print("note: this qwen-asr version has no `context` argument; running Qwen without the hints")
    out = {}
    for it in items:
        kwargs = {"audio": str(it["path"]), "language": None}
        if takes_context and hints["context"]:
            kwargs["context"] = hints["context"]
        t = time.perf_counter()
        r = model.transcribe(**kwargs)[0]
        if device != "cpu":
            torch.cuda.synchronize()
        out[str(it["path"])] = {"text": (r.text or "").strip(), "ms": int((time.perf_counter() - t) * 1000),
                                "lang": getattr(r, "language", None)}
    return out


LLAMA_SERVER = HERE / ".tools" / "llama-b12577" / "llama-server.exe"
LLAMA_PORT = 47391


def run_llamacpp(items: list[dict], hints: dict, cpu: bool, quant: str = "Q8_0") -> dict:
    """Qwen3-ASR 1.7B in llama.cpp (native C++, CUDA): the same model as `qwen`, run the way an app
    on Windows could embed it. Prompt built like qwen-asr: hints as the system message, audio as the user's."""
    import base64
    import urllib.request

    from huggingface_hub import hf_hub_download

    if not LLAMA_SERVER.exists():
        sys.exit(f"llama.cpp not found at {LLAMA_SERVER}")
    repo = "ggml-org/Qwen3-ASR-1.7B-GGUF"
    model = hf_hub_download(repo, f"Qwen3-ASR-1.7B-{quant}.gguf")
    mmproj = hf_hub_download(repo, f"mmproj-Qwen3-ASR-1.7B-{quant}.gguf")
    cmd = [str(LLAMA_SERVER), "-m", model, "--mmproj", mmproj, "--port", str(LLAMA_PORT), "-c", "8192",
           "-ngl", "0" if cpu else "99", "--no-webui"]
    log = open(HERE / ".work" / "llama-server.log", "w", encoding="utf-8")
    server = subprocess.Popen(cmd, stdout=log, stderr=subprocess.STDOUT)
    base = f"http://127.0.0.1:{LLAMA_PORT}"
    try:
        for _ in range(240):
            try:
                with urllib.request.urlopen(base + "/health", timeout=2) as r:
                    if r.status == 200:
                        break
            except Exception:
                if server.poll() is not None:
                    sys.exit("llama-server exited; see .work/llama-server.log")
                time.sleep(0.5)
        out = {}
        for it in items:
            audio = base64.b64encode(Path(it["path"]).read_bytes()).decode()
            messages = [
                {"role": "system", "content": hints["context"] or ""},
                {"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": audio, "format": "wav"}}]},
            ]
            body = json.dumps({"messages": messages, "temperature": 0, "max_tokens": 448}).encode()
            req = urllib.request.Request(base + "/v1/chat/completions", data=body,
                                         headers={"Content-Type": "application/json"})
            t = time.perf_counter()
            with urllib.request.urlopen(req, timeout=120) as r:
                reply = json.loads(r.read())
            ms = int((time.perf_counter() - t) * 1000)
            raw = (reply["choices"][0]["message"].get("content") or "").strip()
            lang, text = None, raw
            if "<asr_text>" in raw:
                head, text = raw.split("<asr_text>", 1)
                lang = head.replace("language", "").strip() or None
            out[str(it["path"])] = {"text": text.strip(), "ms": ms, "lang": lang}
        return out
    finally:
        server.terminate()
        server.wait(timeout=10)
        log.close()


MODELS = {
    "qwen-llamacpp": run_llamacpp,
    "whisper": run_whisper,
    "qwen": run_qwen,
    "qwen-0.6b": lambda items, hints, cpu: run_qwen(items, hints, cpu, "0.6B"),
}


# ---------- scoring ----------


def norm(text: str) -> str:
    text = text.lower().replace("-", " ")
    text = re.sub(r"[^\w\s]", " ", text)
    return re.sub(r"\s+", " ", text).strip()


def echoes(text: str) -> bool:
    """The output is the hints (or the app's example sentence) copied back, not what was said."""
    t = norm(text)
    return ("glossary" in t and "people" in t) or norm(BASE_SENTENCE)[:30] in t


def term_hits(terms: list[str], text: str) -> int:
    t = f" {norm(text)} "
    return sum(t.count(f" {norm(term)} ") for term in terms if norm(term))


def cmd_run(args):
    import jiwer

    set_dir = Path(args.set)
    if not set_dir.is_absolute() and not set_dir.exists():
        set_dir = SETS / args.set
    items = read_set(set_dir)
    if args.from_min is not None or args.to_min is not None:
        lo, hi = (args.from_min or 0) * 60, (args.to_min if args.to_min is not None else 1e9) * 60
        items = [i for i in items if lo <= i.get("start_s", 0) < hi]
    items = items[: args.limit or None]
    glossary = [t.strip() for t in args.glossary.split(",") if t.strip()]
    people = [t.strip() for t in args.people.split(",") if t.strip()]
    context = " ".join(
        p for p in [
            f"Meeting: {args.title}." if args.title else "",
            f"Glossary: {', '.join(glossary)}." if glossary else "",
            f"People: {', '.join(people)}." if people else "",
        ] if p
    )
    hints = {"context": context}
    audio_s = sum(sf.info(str(i["path"])).duration for i in items)
    print(f"{len(items)} clips, {audio_s / 60:.1f} min of audio")

    results = {}
    for name in args.models.split(","):
        saved = set_dir / f"results-{name}.jsonl"
        if args.report_only:
            by_wav = {json.loads(l)["wav"]: json.loads(l) for l in saved.read_text(encoding="utf-8").splitlines() if l}
            results[name] = {str(i["path"]): by_wav.get(i["wav"], {"text": "", "ms": 0}) for i in items}
            continue
        print(f"running {name}...")
        t = time.perf_counter()
        results[name] = MODELS[name](items, hints, args.cpu)
        wall = time.perf_counter() - t
        # Merge into earlier results, so a run over part of the set keeps the rest.
        merged = {}
        if saved.exists():
            merged = {json.loads(l)["wav"]: json.loads(l) for l in saved.read_text(encoding="utf-8").splitlines() if l}
        merged.update({i["wav"]: {"wav": i["wav"], **results[name][str(i["path"])]} for i in items})
        saved.write_text(
            "\n".join(json.dumps(merged[k], ensure_ascii=False) for k in sorted(merged)),
            encoding="utf-8",
        )
        print(f"  done in {wall:.0f} s including model load")

    clip_refs = [i.get("ref") for i in items]
    # An empty correct text means nothing was said in that clip.
    has_clip_refs = bool(clip_refs) and all(r is not None for r in clip_refs)
    whole_ref = (set_dir / "ref.txt").read_text(encoding="utf-8") if (set_dir / "ref.txt").exists() else None
    terms = glossary + people

    lines = [f"# Speech model comparison: {set_dir.name}", "",
             f"{len(items)} clips, {audio_s / 60:.1f} min of audio. Hints given to every model: "
             f"{context or '(none)'}", ""]
    header = "| Model | Word errors | Letter errors | Speed (x real time) |"
    sep = "|---|---|---|---|"
    if terms:
        header += " Glossary/names found |"
        sep += "---|"
    lines += [header, sep]
    term_total = None
    if terms:
        refs_text = " ".join(clip_refs) if has_clip_refs else whole_ref
        term_total = term_hits(terms, refs_text) if refs_text else None
    tracks = sorted({i.get("track", "") for i in items})
    if len(tracks) > 1:
        header += " Blank (" + " / ".join(t.rsplit("-", 1)[-1] for t in tracks) + ") |"
        sep += "---|"
    header += " Echoed the hints |"
    sep += "---|"
    lines[-2:] = [header, sep]
    for name, res in results.items():
        raw = [res[str(i["path"])]["text"] for i in items]
        # A model that copies its hints into the output heard nothing; count it and score it as blank.
        echoed = [echoes(h) for h in raw]
        hyps = ["" if e else h for h, e in zip(raw, echoed)]
        busy_s = sum(res[str(i["path"])]["ms"] for i in items) / 1000
        if has_clip_refs:
            # Joined in clip order: scoring per clip would fail on clips where nothing was said.
            refs_n, hyps_n = [norm(" ".join(clip_refs))], [norm(" ".join(hyps))]
        elif whole_ref:
            refs_n, hyps_n = [norm(whole_ref)], [norm(" ".join(hyps))]
        else:
            refs_n = None
        wer = f"{jiwer.wer(refs_n, hyps_n):.1%}" if refs_n else "n/a (no correct text)"
        cer = f"{jiwer.cer(refs_n, hyps_n):.1%}" if refs_n else "n/a"
        speed = f"{audio_s / busy_s:.0f}x" if busy_s else "?"
        row = f"| {name} | {wer} | {cer} | {speed} |"
        if terms:
            found = term_hits(terms, " ".join(hyps))
            row += f" {found}" + (f" of {term_total}" if term_total is not None else "") + " |"
        if len(tracks) > 1:
            row += " " + " / ".join(
                str(sum(1 for i, h in zip(items, hyps) if i.get("track") == t and not h.strip())) for t in tracks
            ) + " |"
        row += f" {sum(echoed)} |"
        lines.append(row)
    lines += ["", "Lower error is better. Without correct text, compare the lines below by ear.", "",
              "## Side by side", ""]
    names = list(results)
    lines += ["| # | " + ("Correct | " if has_clip_refs else "") + " | ".join(names) + " |",
              "|---|" + ("---|" if has_clip_refs else "") + "---|" * len(names)]
    cell = lambda s: (s or "").replace("|", "\\|").replace("\n", " ")
    for n, it in enumerate(items[: args.show]):
        label = str(n)
        if "start_s" in it:
            s = it["start_s"]
            label += f" · {int(s // 60)}:{int(s % 60):02d} {it.get('track', '').rsplit('-', 1)[-1]}"
        row = f"| {label} | " + (f"{cell(it['ref'])} | " if has_clip_refs else "")
        row += " | ".join(cell(results[m][str(it['path'])]["text"]) for m in names) + " |"
        lines.append(row)
    (set_dir / "report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print("\n".join(lines[:10 + len(results)]))
    print(f"\nfull report: {set_dir / 'report.md'}")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="cmd", required=True)

    f = sub.add_parser("fleurs", help="download public Indonesian clips with correct text")
    f.add_argument("--n", type=int, default=40)
    f.add_argument("--name", default="fleurs-id")
    f.set_defaults(fn=cmd_fleurs)

    r = sub.add_parser("record", help="record your PC's audio and mic until Ctrl+C")
    r.add_argument("--name", required=True)
    r.add_argument("--minutes", type=float)
    r.set_defaults(fn=cmd_record)

    pr = sub.add_parser("prepare", help="cut recordings into clips")
    pr.add_argument("audio", nargs="+")
    pr.add_argument("--name", required=True)
    pr.add_argument("--ref", help="a text file with the correct transcript of the whole recording")
    pr.set_defaults(fn=cmd_prepare)

    dr = sub.add_parser("draft", help="write a file for typing the correct text of a stretch")
    dr.add_argument("set")
    dr.add_argument("--from-min", type=float, required=True)
    dr.add_argument("--to-min", type=float, required=True)
    dr.add_argument("--out", help="file name to write (default correct-<from>-<to>.txt)")
    dr.add_argument("--keep", help="an earlier draft whose typed answers to carry over")
    dr.set_defaults(fn=cmd_draft)

    im = sub.add_parser("import", help="read the correct text back from a draft file")
    im.add_argument("set")
    im.add_argument("file")
    im.set_defaults(fn=cmd_import)

    ru = sub.add_parser("run", help="run the models on a set and write the report")
    ru.add_argument("set")
    ru.add_argument("--models", default="whisper,qwen")
    ru.add_argument("--title", default="")
    ru.add_argument("--glossary", default="")
    ru.add_argument("--people", default="")
    ru.add_argument("--limit", type=int, default=0)
    ru.add_argument("--show", type=int, default=40, help="clips shown side by side in the report")
    ru.add_argument("--cpu", action="store_true")
    ru.add_argument("--report-only", action="store_true", help="rebuild the report from saved results")
    ru.add_argument("--from-min", type=float, help="only clips starting at or after this minute")
    ru.add_argument("--to-min", type=float, help="only clips starting before this minute")
    ru.set_defaults(fn=cmd_run)

    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
