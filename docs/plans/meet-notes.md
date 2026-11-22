# Meet: transcripts, Bangla/English subtitles and AI notes

The user asked on 2026-11-22 for an "auto conversation extractor and auto note using ai on meet
section per time line and attach conversation as subtitle with time frame both bangla and
english language using smaller python or llm fit in it".

Every recorded meeting gets, on this PC only, with nothing sent anywhere:

1. **A transcript on a timeline:** who said what, when, in the language they spoke (Bangla or
   English, mixed meetings too).
2. **Subtitles in both languages** attached to the recording: Bangla (`.bn.vtt`) and English
   (`.en.vtt`), and the original mixed track. Zazabor's player shows them and switches between
   them. They're also saved beside the `.webm` for any player.
3. **AI notes:**
   - a summary
   - decisions
   - action items (who, what, when)
   - open questions
   - a chapter list ("key moments") with timestamps that jump into the recording

## Where it shows

Meet → Recordings → a meeting opens a **meeting page**:
- the player (with the subtitle language switch)
- the **Transcript** tab: speaker names, times, the spoken language, and click a line to jump there; search
- the **Notes** tab: the summary, decisions, actions, questions and chapters, editable
- export: Markdown, a Notes note in Zazabor's Notes section, or a Docs document
- copy as e-mail to the invitees through Mail

The work runs in the background after the meeting ends, with progress on the card ("Transcribing
12 of 48 min…", "Writing notes…"). It can be re-run with a bigger model, and per meeting it can be
switched off. Settings → Meet: "Transcribe and take notes automatically" (on), the model size, the
languages, and "delete transcripts with the recording".

## How it works

### Who said what: per-speaker tracks (no guessing)

The host's meeting page already receives each participant's audio as a separate WebRTC track.
Besides the mixed `.webm`, it records **one audio-only track per participant** (MediaRecorder,
Opus, 2 s chunks, uploaded like the video). Each carries the participant's name and the offset of
its start in the meeting, and goes to `<recording>.tracks/<n>-<name>.webm`. Transcribing each
track separately gives exact speaker labels, and overlaps are kept. No diarization model is
needed. The host's own mic is a track too.

### Speech to text: the Whisper engine Zazabor already has

`src-tauri/src/videoedit/whisper/` (Rust on candle, whisper.cpp ggml model files, on the CPU)
already transcribes, with language detection and word times, for Studio's auto captions. Meet
reuses it:

- Moved to a shared place (`src-tauri/src/ai/whisper/`) so both Studio and Meet use it without
  copying.
- **Bangla:** Whisper `small` handles English well but Bangla only fairly. `medium` (1.5 GB) is
  clearly better for Bangla, and runs at about real time on 12 CPU cores. The default is: `small`
  when the meeting language is English, `medium` when Bangla is detected or chosen. Downloads come
  from the same place Studio's do, with a checksum, on first use.
- Code-switching (Bangla and English in one meeting): language detection per 30 s window,
  so segments keep their own language.
- Add Whisper's **translate** task (any language → English) for the English track of Bangla
  speech.
- Optional later: a Bangla-fine-tuned Whisper converted to ggml, if one with a clear licence
  exists.

### Translation and notes: a small local LLM

- **Qwen2.5-3B-Instruct, quantized (GGUF Q4_K_M, about 2 GB)**, run with candle's quantized Qwen2
  support (`candle-transformers` 0.11 is already in the offline cache). It's multilingual, Bangla
  included. The GTX 1650 (4 GB) could speed it up with candle's CUDA backend later; it's CPU
  first.
- **English → Bangla subtitles** (for what was spoken in English): the LLM translates segment by
  segment with the speaker context, keeping the timing.
- **Bangla → English:** Whisper translate first, with the LLM as a fallback.
- **Notes:** the transcript is cut into chapters of about 3–5 minutes. Each chapter gets a short
  summary and a title (map). Then one pass writes the overall summary, decisions, actions with
  owners (speaker names are known) and questions (reduce). The output is strict JSON, checked and
  repaired. Notes are written in English and Bangla (the user picks one, or both).
- A 1-hour meeting on this CPU should take about 15–25 min for speech-to-text (medium) and
  5–10 min for notes. It runs at low priority (nice 10, at most 6 threads), pauses while the PC is
  busy (a video playing, a game, a build), and can be cancelled.
- **Optional:** "Use Claude for better notes" (Zazabor already talks to Claude for Statements).
  Off by default. When on, it sends the transcript text, never audio, and asks first.

### Files

Beside the recording:
- `<name>.webm`
- `<name>.tracks/`
- `<name>.transcript.json` (segments: speaker, start, end, lang, text, en, bn)
- `<name>.bn.vtt`, `<name>.en.vtt`, `<name>.vtt` (original)
- `<name>.notes.json` and `<name>.notes.md`

They're deleted with the recording, unless kept.

## Phases

1. **Per-speaker tracks in the meeting page + the transcription job + subtitles + the meeting
   page with the transcript.** Shared Whisper; small/medium; Bangla/English detection; translate
   to English.
2. **LLM: English→Bangla subtitles + notes (summary, decisions, actions, questions, chapters) +
   export.**
3. **Live captions during the meeting** (Whisper on rolling 10 s windows of each track, shown
   to everyone; English/Bangla) and a "Catch me up" summary for late joiners.

## Testing

- Short clips only (≤ 10 s each, per the user's rule), with synthetic two-speaker meetings built
  from short Bangla and English sample clips. TTS isn't available offline, so a small set of CC0 /
  public-domain Bangla and English speech clips, checked into test fixtures if small, or
  downloaded in an ignored test.
- Word error rate is checked by ear and by sample on the real Bangla meeting the user records
  first.
