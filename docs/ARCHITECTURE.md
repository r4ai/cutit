# ARCHITECTURE.md — Minimal NLE MVP (Rust + FFmpeg + iced)

This document defines a **minimal MVP** non-linear editor (NLE) implemented in **Rust** with **FFmpeg** for media I/O and a native GUI built with **iced**.

The MVP supports:

- **Import**: open a single media file and inspect streams
- **Cut editing**: hard cuts only (split + optional ripple delete)
- **Preview**: scrubbing (frame preview at playhead time)
- **Export**: render the edited timeline to a new file via **decode → re-timestamp → encode → mux**

Non-goals for MVP:
- No titles/captions, no compositing, no effects, no masks, no keyframes
- No transitions
- No multi-track / overlays
- No proxy workflow (optional later)
- No color management (OCIO), no EXR/linear compositing
- No plugin system (OFX etc.)

Primary constraints:
- **Engine is UI-agnostic** (GUI implementation is isolated in `crates/ui`)
- **FFmpeg contexts are confined to worker threads** (no sharing across threads)
- Prefer **correctness & determinism** over “smart” stream copy

---

## 0. MVP scope levels (explicit)

This project can be delivered in two incremental “MVP levels”:

### MVP-0 (video-only)
- Import one video file
- Cut edit (split)
- Preview frames while scrubbing
- Export **video-only** output (no audio)

### MVP-1 (video + audio)
- All of MVP-0
- Export with audio (decode → resample if needed → timestamped encode)
- Enforce A/V sync by a single export timeline clock

This document describes MVP-1; implement MVP-0 first if schedule/risk requires.

---

## 1. High-level architecture

```
+-------------------+        commands/events        +------------------------+
|       UI          | <---------------------------> |     Engine API         |
|    (ui crate)     |                               | (UI-agnostic crate)    |
+---------+---------+                               +-----------+------------+
          |                                                     |
          | preview frames                                      | uses
          v                                                     v
+-------------------+                               +------------------------+
| Preview Renderer  |<-- frames (RGBA/NV12) --------| Media Pipeline (FFmpeg) |
|   (ui widget)     |                               | probe/demux/decode/... |
+-------------------+                               +------------------------+
```

**Core principle**: UI never touches FFmpeg. The UI speaks only in:
- timeline time `t`
- editing commands (split, delete)
- project snapshots (for rendering timeline)
- preview frames delivered by the engine

---

## 2. Repository / crate layout (suggested)

```
/crates
  /engine
    src/
      lib.rs
      api.rs              # UI-facing API: Engine, commands, events
      project.rs          # Project model + serde persistence
      timeline.rs         # Timeline data + time mapping
      preview.rs          # Preview scheduling + caching policy
      export.rs           # Export: decode -> retimestamp -> encode -> mux
      cache.rs            # LRU caches for preview frames / seek contexts
      time.rs             # Rational/ticks utilities + safe comparisons
      error.rs
  /media-ffmpeg
    src/
      lib.rs
      probe.rs            # open input, discover streams, extract metadata
      demux.rs
      decode.rs
      encode.rs
      mux.rs
      resample.rs         # audio resampling (libswresample)
      scale.rs            # pixel format conversion (libswscale) if needed
      time.rs             # FFmpeg time_base helpers (rescale wrappers)
      error.rs
  /ui
    src/
      main.rs             # UI entrypoint (application builder)
      app.rs              # App state + Message + update/view/subscription
      widgets/
        preview.rs        # Preview widget (Image / shader path)
        timeline.rs       # Timeline widget (Canvas) + interaction
      bridge.rs           # Engine bridge (channels, subscriptions)
  /cli (optional)
    src/
      main.rs             # headless import/export tool (tests/regression)
/docs
  ARCHITECTURE.md
```

Notes:
- `engine` depends on `media-ffmpeg` via traits (recommended) or directly (acceptable for MVP).
- `ui` depends on `engine` only (no FFmpeg types, no `media-ffmpeg`).
- `cli` is strongly recommended as a deterministic regression harness for export correctness.

### 2.1 GUI implementation boundaries (where to implement)
- Put all GUI code in `crates/ui/src/` only.
- `crates/ui/src/main.rs`: app bootstrap + subscription wiring.
- `crates/ui/src/app.rs`: `AppState`, `Message`, `update`, `view`.
- `crates/ui/src/bridge.rs`: command/event bridge with the engine thread.
- `crates/ui/src/widgets/preview.rs`: preview surface and frame presentation.
- `crates/ui/src/widgets/timeline.rs`: timeline drawing + scrub/split interactions.
- Keep GUI concerns out of `crates/engine` and `crates/media-ffmpeg`.

---

## 3. Time model (critical for correctness)

### 3.1 Requirements
- Stable mapping between:
  - **timeline time** (editor domain)
  - **source media time** per stream (`time_base`)
  - **output time** per encoder (`time_base`)
- Must handle non-integer frame rates (e.g., 30000/1001) and audio sample rates.

### 3.2 Representation
**Do not represent all time as “seconds rational” everywhere.**
Use an FFmpeg-like “ticks + time_base” model for safety and simplicity.

```rust
/// FFmpeg-like timestamp representation.
pub struct Ticks {
  pub ts: i64,              // integer timestamp in units of `time_base`
  pub time_base: (i32, i32) // numerator/denominator (AVRational)
}
```

Guidelines:
- All conversions between time bases MUST use a rescale function equivalent to FFmpeg `av_rescale_q`.
- All comparisons must avoid overflow (use i128 for cross-multiply if needed).
- Timeline time uses a fixed base (e.g., 1/1_000_000 seconds) to simplify editing math.

Recommended constants:
- `TIMELINE_TIME_BASE = (1, 1_000_000)` (microseconds)
- `AUDIO_CLOCK_BASE` derived from output sample rate (see §7.4)

### 3.3 Rescale API (engine-side contract)
Provide a single “truth” function for conversion:

```rust
pub fn rescale(ts: i64, from: (i32, i32), to: (i32, i32)) -> i64;
```

The `media-ffmpeg` crate implements it via FFmpeg utilities; the `engine` calls it exclusively.

---

## 4. Data model (MVP)

### 4.1 Project
```rust
pub struct Project {
  pub assets: Vec<MediaAsset>,
  pub timeline: Timeline,
  pub settings: ProjectSettings,
}
```

### 4.2 MediaAsset
```rust
pub struct MediaAsset {
  pub id: AssetId,
  pub path: std::path::PathBuf,
  pub video: Option<VideoStreamInfo>,
  pub audio: Option<AudioStreamInfo>,
  pub duration_tl: i64, // duration in TIMELINE_TIME_BASE ticks
}
```

Stream info caches the stream `time_base`, codec parameters, and basic metadata.

### 4.3 Timeline (single track)
MVP timeline is a single linear sequence of segments. No overlaps.

```rust
pub struct Timeline {
  pub segments: Vec<Segment>,  // sorted by timeline_start
}

pub struct Segment {
  pub id: SegmentId,
  pub asset_id: AssetId,

  // source range in source stream timebase *per stream*
  pub src_in_video: Option<i64>,
  pub src_out_video: Option<i64>,
  pub src_in_audio: Option<i64>,
  pub src_out_audio: Option<i64>,

  // segment start in timeline base (TIMELINE_TIME_BASE)
  pub timeline_start: i64,
  pub timeline_duration: i64,
}
```

Invariant (MVP):
- `segments` are contiguous (no gaps) unless explicitly allowed.
- Cuts are exactly segment boundaries.
- `timeline_duration` is authoritative; it determines export length.

### 4.4 Editing operations (MVP)
- `Split(at_tl)`:
  - find segment containing `at_tl`
  - compute `src_at` for video/audio by mapping `at_tl` into source time bases
  - replace with two segments, adjusting `src_in/out` and `timeline_duration`
- `RippleDelete(range_tl)` (optional):
  - remove portions, shift later segments left to close gaps

---

## 5. Engine API (UI-facing)

### 5.1 Commands and events
UI issues commands; engine emits events (including progress and errors).

```rust
pub enum Command {
  Import { path: PathBuf },

  SetPlayhead { t_tl: i64 },    // timeline ticks, clamped to [0, duration_tl - 1]
  Split { at_tl: i64 },

  Export { path: PathBuf, settings: ExportSettings },
  CancelExport,
}

pub enum Event {
  ProjectChanged(ProjectSnapshot),
  PlayheadChanged { t_tl: i64 },

  PreviewFrameReady { t_tl: i64, frame: PreviewFrame },

  ExportProgress { done: u64, total: u64 },
  ExportFinished { path: PathBuf },

  Error(EngineErrorEvent),
}
```

### 5.2 Snapshots
UI renders from immutable snapshots (thread-safe, no FFmpeg types).

```rust
pub struct ProjectSnapshot {
  pub assets: Vec<MediaAssetSummary>,
  pub segments: Vec<SegmentSummary>,
  pub duration_tl: i64,
}
```

### 5.3 PreviewFrame contract (UI-agnostic)
To keep UI-agnosticism, preview frames are raw pixels + metadata.

```rust
pub enum PreviewPixelFormat {
  Rgba8,   // MVP default
  Nv12,    // optional optimization path
}

pub struct PreviewFrame {
  pub width: u32,
  pub height: u32,
  pub format: PreviewPixelFormat,
  pub bytes: std::sync::Arc<[u8]>,
}
```

UI converts `PreviewFrame` to an `iced` renderable object (e.g., an `Image` handle for RGBA; or a shader path for NV12).

---

## 6. Concurrency model (required for correctness & responsiveness)

### 6.1 Threads
- **UI thread**: iced event loop + rendering + input dispatch
- **Engine thread**: owns `Project`, applies edits, schedules work
- **Preview worker**: owns FFmpeg demux/decode contexts for preview
- **Export worker**: owns FFmpeg demux/decode/encode/mux contexts for export

**FFmpeg contexts are not shared across threads**.
All FFmpeg objects live and die within their owning worker thread.

### 6.2 Communication
- Use channels with explicit backpressure where appropriate.
- Preview requests are **coalesced**: only the newest request is processed during scrubbing.

### 6.3 Cancellation
- Preview: coalescing effectively cancels older requests.
- Export: cancellation via atomic flag + control channel message.

---

## 7. Media pipeline (FFmpeg) — precise semantics

This section fixes the previously ambiguous parts: seek, timestamps, and export retimestamping.

### 7.1 Import / probe
- Open input container (demux) and enumerate streams.
- Choose “best” video/audio streams (MVP: first suitable).
- Capture:
  - codec id, pixel/sample formats
  - stream `time_base` (per stream)
  - duration (if available) and start time
  - resolution / sample_rate / channels

Store metadata in `MediaAsset`.

### 7.2 Preview (decode-only): seek + decode-forward
Goal: for a requested playhead time `t_tl` (timeline ticks), produce a preview frame.

**Mapping**
1. Locate segment `S` containing `t_tl`
2. Compute `local_t = t_tl - S.timeline_start`
3. Convert to source time:
   - `src_target_video_ts = S.src_in_video + rescale(local_t, TIMELINE_TIME_BASE, video_time_base)`

**Seek strategy**
- Seek to a timestamp <= `src_target_video_ts` (ideally keyframe-aligned).
- After seek:
  - flush demuxer/decoder state (FFmpeg-style flush is mandatory)
  - decode forward until the target PTS is reached

**Timestamp semantics**
- Use **presentation timestamp (PTS)** for target matching.
- Use the frame’s **best-effort timestamp** when PTS is missing/unstable.

**Frame selection rule (video)**
- Emit the first decoded frame where:
  - `frame_pts >= src_target_video_ts` (in the video stream time base)

**Pixel format for UI**
- MVP default: convert to **RGBA** (CPU conversion acceptable at first)
- Optimization path: deliver **NV12** and do YUV→RGB in GPU shader

**Caching**
- LRU cache by `(asset_id, size, coarse_bucket(src_target_video_ts))`
- Additionally cache “seek contexts” (decoder warmed near certain regions) to speed scrubbing.

### 7.3 Export: decode → retimestamp → encode → mux
We explicitly choose re-encode for correctness and simplicity.

Export iterates segments in timeline order, producing a single continuous output.

#### 7.3.1 Output formats (MVP defaults)
- Container: MP4
- Video: H.264 (preferred), otherwise fail fast (MVP) unless alternative configured
- Audio: AAC
- Output starts at `t=0` (timeline time)

#### 7.3.2 Timeline clock (single source of truth)
Define `out_time_tl` as the current export time in timeline ticks.

For each segment `S`, define:
- `segment_out_start_tl = S.timeline_start`
- For any source timestamp `src_pts` (in source stream time base), define its timeline-relative time:
  - `src_offset_tl = rescale(src_pts - S.src_in_*, src_time_base, TIMELINE_TIME_BASE)`
  - `out_time_tl = S.timeline_start + src_offset_tl`

Then produce output timestamps by rescaling `out_time_tl` into encoder time bases.

#### 7.3.3 Video retimestamping (PTS generation)
- Decode source video frames; for each frame, compute:
  - `out_pts_video = rescale(out_time_tl, TIMELINE_TIME_BASE, out_video_time_base)`
- Only include frames whose source PTS is within `[src_in_video, src_out_video)`.

**Important**:
- Use PTS ordering for display correctness (B-frames).
- Do not reuse source DTS directly; let the encoder decide reordering.
- Enforce monotonic output PTS (if equal/descending, bump minimally per encoder constraints).

#### 7.3.4 Audio policy (MVP correctness-first)
Audio is handled in a way that avoids drift.

Preferred MVP policy:
- Decode audio to a common sample format.
- Resample to the output sample rate (e.g., 48kHz) if required.
- Output audio PTS is generated from **accumulated output sample count**, not copied from source.

Define:
- `out_audio_sample_rate` (e.g., 48000)
- `out_audio_time_base = (1, out_audio_sample_rate)`
- Maintain `audio_samples_emitted: i64`

For each emitted audio frame with `n` samples:
- `out_pts_audio = audio_samples_emitted` (in `out_audio_time_base`)
- `audio_samples_emitted += n`

This guarantees continuous audio timeline without drift from imperfect source timestamps.

To cut audio to match segments:
- Include decoded audio samples covering `[src_in_audio, src_out_audio)`.
- Trim partial audio frames at boundaries (sample-accurate trimming).

#### 7.3.5 VFR policy (MVP)
MVP **preserves VFR semantics** by propagating timestamps through the timeline mapping (as above).
CFR conversion (drop/duplicate frames) is explicitly out-of-scope for MVP.

---

## 8. UI architecture with iced (MVP GUI)

### 8.1 iced version + feature policy (pin explicitly)
Pin an explicit iced minor version in `ui/Cargo.toml` to avoid silent breakage. Use a workspace lockfile.

Minimum feature set for this MVP:
- `image` for preview display (RGBA path)
- `canvas` for a custom timeline widget (segments + playhead + hit-testing)
- `advanced` for direct RGBA handle construction (avoid on-disk codecs)
- a renderer backend:
  - keep defaults for development; optionally disable unused backends later
- pick executor backend explicitly:
  - keep default executor **or** enable `tokio` if you need tokio interop for tasks (file dialogs, sockets, etc.)

Example (illustrative; adjust to your workspace policies):
```toml
[dependencies]
iced = { version = "0.14", default-features = true, features = ["image", "canvas", "advanced"] }
# optional:
# iced = { version = "0.14", default-features = true, features = ["image", "canvas", "advanced", "tokio"] }

tracing = "0.1"
rfd = "0.15"         # file dialogs (UI-side only)
```

### 8.2 iced app structure (Elm architecture)
Implement the UI using iced’s “boot / update / view” application builder.

Key rules for this MVP:
- `AppState` stores only UI state + immutable `ProjectSnapshot` + render caches (preview image handle, timeline cache)
- `Message` is split into:
  - UI messages (button clicks, slider scrubs, timeline interactions)
  - Bridge messages (e.g. `BridgeEvent::Ready`, `BridgeEvent::Event(engine::Event)`, `BridgeEvent::Disconnected`)
- `update` is synchronous and returns `Task<Message>`:
  - usually `Task::none()` when only mutating state / sending a command
  - use tasks for things like file dialogs or long-running UI-side IO
- `subscription` keeps the engine event stream alive and feeds it into `Message::Bridge(...)` (no timer-based polling)

Skeleton:
```rust
pub fn main() -> iced::Result {
  iced::application("Cutit", AppState::update, AppState::view)
    .subscription(AppState::subscription)
    .run_with(AppState::boot)
}
```

### 8.3 Engine bridge: channels + Subscription (robust pattern)
The bridge must satisfy:
- UI can send `engine::Command` immediately (non-blocking)
- UI receives `engine::Event` as an iced `Subscription`
- no FFmpeg types cross the boundary
- engine thread lifetime is controlled by subscription lifetime (MVP convenience)
- command/event channels are bounded to enforce backpressure

Recommended bridge (conceptual):
- Create an **engine command sender** stored in `AppState` after initialization.
- Create a long-lived `Subscription` that:
  1) spawns the engine thread (once)
  2) forwards engine events into iced messages

Practical pattern:
- On app start, `AppState` has `engine_tx: Option<EngineCommandSender> = None`.
- `subscription` starts a worker that emits:
  - `Message::Bridge(BridgeEvent::Ready(engine_tx))` once
  - `Message::Bridge(BridgeEvent::Event(event))` for subsequent events
  - `Message::Bridge(BridgeEvent::Disconnected)` when the bridge stops

This lets `update` be purely synchronous:
- if `engine_tx.is_some()` → send command
- if not ready yet → keep only the newest scrub request (coalescing)

### 8.4 Preview widget (RGBA-first, GPU path later)
**MVP default**: engine delivers `PreviewFrame { format: Rgba8, bytes }`.

UI conversion:
- Convert `PreviewFrame` to an iced image handle (decoded pixels, no codec step).
- Store the resulting handle in UI state (e.g., `Option<iced::advanced::image::Handle>` or `Option<iced::widget::image::Handle>` depending on your chosen API surface).
- Render with `Image` widget; preserve aspect ratio; optionally use a `Container` to letterbox.

Performance notes (still MVP-safe):
- Scrubbing can trigger many frames. Keep **only the latest** preview handle.
- Optional: downscale preview in engine to a fixed maximum size to bound upload bandwidth.

**Post-MVP optimization**:
- Accept `NV12` in `PreviewFrame` and use a GPU shader widget path (YUV→RGB in wgpu) to reduce CPU conversion cost.

### 8.5 Timeline widget (Canvas)
Use `Canvas` for:
- drawing segment rectangles proportional to `timeline_duration`
- drawing playhead line at `t_tl`
- hit-testing clicks/drags:
  - click → set playhead
  - drag → scrub
  - keypress/click at playhead → split

MVP interaction model:
- timeline emits `Message::TimelineScrubbed(t_tl)`
- UI update coalesces scrub updates and sends at most one in-flight `Command::SetPlayhead { t_tl }`
- UI playhead and timeline slider both clamp to `[0, duration_tl - 1]` (when `duration_tl > 0`)
- engine emits `PreviewFrameReady { t_tl, frame }` asynchronously

### 8.6 File dialogs + export progress UI (Tasks + events)
File dialogs should live in UI (engine remains pure):
- `Import` button triggers a **Task** that opens a file dialog (e.g., `rfd`).
- When a path is selected, `update` sends `Command::Import { path }`.

Export:
- `Export` button sends `Command::Export { ... }`
- UI displays progress from `Event::ExportProgress { done, total }`
- `Cancel` button sends `Command::CancelExport`

---

## 9. Persistence (MVP)

- Project file: `project.json` (or `*.nle.json`) via `serde`.
- Persist:
  - asset file paths
  - stream selection (video/audio stream indices)
  - segments (src_in/out, timeline_start/duration)
  - export settings (optional)
- Do not embed media or proxies in MVP.

---

## 10. Error handling & logging

- Typed errors with `thiserror`
- Structured logs with `tracing`
- Surface user-facing errors to UI via `Event::Error`
- Log-first policy: use appropriate log levels (`error`, `warn`, `info`, `debug`, `trace`)
- Log-first policy: include debugging context in logs (asset/segment IDs, timestamps, time_base)
- Log-first policy: prefer structured fields over string concatenation

Common failure modes:
- Unsupported codec / decoder missing
- Seek imprecision / broken timestamps (handled by decode-forward + best-effort PTS)
- Missing encoder (H.264/AAC not available in FFmpeg build)
- Short reads / corrupt media

---

## 11. Testing strategy (test-first)

- Write tests first to clarify the spec before implementation.
- Keep tests minimal but sufficient to guarantee the intended behavior.
- Separate side effects from logic to keep core components easy to test.
- Focus regression coverage on:
  - Timebase conversions (`rescale`), timeline mapping, and segment boundaries
  - Export correctness (duration, monotonic PTS/DTS) via the CLI harness

---

## 12. FFmpeg dependency & distribution strategy (must be decided early)

Pick one strategy:
1) **System FFmpeg** during development (simplest)
2) **Bundled FFmpeg shared libs** for distribution (common)
3) **Static linking** (harder; platform-specific constraints)

MVP should explicitly fail with a clear error if required encoders/decoders are absent.

Note: `ffmpeg-next` exists and offers Rust wrappers; however it has been described as in maintenance mode, so evaluate `rsmpeg` or direct bindings if long-term maintenance is a priority.

---

## 13. Implementation plan (incremental, testable)

### Step 1 — Media probe + single-frame decode
- `media-ffmpeg`: open file, detect streams, decode a frame near time `t`
- Validate timestamp reading (`best_effort_timestamp`) and time base conversions

### Step 2 — Timeline core + scrubbing
- `engine`: create `Project` with one segment covering full duration
- Implement `SetPlayhead` → preview request → `PreviewFrameReady`
- Implement `Split`

### Step 3 — Export video-only (MVP-0)
- Export pipeline: iterate timeline segments
- Retimestamp video frames using timeline mapping
- Encode + mux

### Step 4 — Add audio export (MVP-1)
- Decode audio, trim at boundaries, resample if needed
- Generate output audio PTS from sample counter
- Ensure muxed A/V sync

### Step 5 — Regression harness
- CLI tool that:
  - imports a file
  - applies deterministic cuts
  - exports
  - validates output duration and monotonic timestamps via ffprobe-like inspection

### Step 6 — GUI bootstrap + engine bridge
Where to implement:
- `crates/ui/src/main.rs`
- `crates/ui/src/app.rs`
- `crates/ui/src/bridge.rs`

Work:
- Bootstrap the application (`boot/update/view/subscription`)
- Initialize engine thread and command/event channels
- Wire basic UI actions: Import, SetPlayhead, Split command dispatch
- Use a subscription-based bridge event flow (no fixed-interval polling)
- Add bounded channels and scrub command coalescing to avoid command/event backlog

### Step 7 — GUI widgets (preview + timeline interaction)
Where to implement:
- `crates/ui/src/widgets/preview.rs`
- `crates/ui/src/widgets/timeline.rs`
- `crates/ui/src/app.rs`

Work:
- Render latest `PreviewFrame`
- Draw segments + playhead on timeline
- Implement click/drag scrub and split-triggered message flow

### Step 8 — Cut feature hardening (engine + UI)
Where to implement:
- `crates/engine/src/timeline.rs`
- `crates/engine/src/api.rs`
- `crates/ui/src/app.rs`
- `crates/ui/src/widgets/timeline.rs`

Work:
- Keep `Step 2/6/7` implementations as baseline and define failing tests first for cut edge cases
- Harden `Split { at_tl }` behavior at boundaries (start/end, segment edges, mid-segment) while preserving timeline invariants
- Verify `ProjectChanged` snapshot consistency after repeated cuts and no-op conditions (out-of-range / invalid points)
- Finalize UI cut UX (playhead-based trigger, selection feedback, command dispatch/result reflection)
- Add structured logs for cut operations (segment IDs, timeline ticks, mapped source timestamps)
- Add/refresh doc-comments and examples on cut-related public APIs

### Step 9 — Export feature integration & hardening
Where to implement:
- `crates/engine/src/export.rs`
- `crates/engine/src/api.rs`
- `crates/media-ffmpeg/src/{decode.rs,encode.rs,mux.rs,resample.rs,time.rs}`
- `crates/ui/src/app.rs`
- `crates/cli/src/main.rs`

Work:
- Define failing tests first for export correctness (duration, monotonic timestamps, A/V sync)
- Integrate existing export pipeline into end-to-end flow (`Export`, `CancelExport`, progress events, UI state transitions)
- Harden cancellation, error propagation, and worker-thread boundaries under long-running exports
- Validate timeline-driven retimestamping against segment boundaries for cut-then-export cases
- Extend deterministic CLI regression checks for cut-then-export scenarios (video-only and video+audio paths)
- Add structured logs for export progress/failures and refresh doc-comments/examples on export-related public APIs

---

## 14. Post-MVP extension points
- Proxy generation (background transcode)
- Waveform/peaks for audio visualization
- Multi-track timeline
- Basic effects (transform/opacity) with wgpu render graph
- OTIO import/export at project boundaries (JSON/.otio parsing)

---
