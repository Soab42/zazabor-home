//! Makes a recording seekable. The meeting page's MediaRecorder writes a live WebM: no duration and
//! no cue index, so players show no length and can't seek (the user saw a disabled seek bar). After a
//! recording ends, GStreamer copies its streams unchanged (no re-encoding) into a new WebM, whose muxer
//! writes the duration and cues at the end; the new file then replaces the old one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;

/// The longest a remux may take (it only copies; an hour-long meeting takes seconds).
const TIMEOUT: Duration = Duration::from_secs(20 * 60);

fn init() -> Result<(), String> {
    static INIT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    INIT.get_or_init(|| gst::init().map_err(|e| e.to_string())).clone()
}

/// The temporary file next to `path` while it's being rewritten.
fn part_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".seekable-part");
    path.with_file_name(name)
}

/// Rewrites `path` in place with a duration and cues. The original stays untouched if anything fails.
pub fn make_seekable(path: &Path) -> Result<(), String> {
    init()?;
    let part = part_path(path);
    let _ = std::fs::remove_file(&part);
    let result = remux(path, &part);
    match result {
        Ok(()) => {
            let size = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
            if size == 0 {
                let _ = std::fs::remove_file(&part);
                return Err("the rewritten file came out empty".into());
            }
            std::fs::rename(&part, path).map_err(|e| format!("couldn't replace the recording: {e}"))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            Err(e)
        }
    }
}

fn remux(input: &Path, output: &Path) -> Result<(), String> {
    let make = |factory: &str| gst::ElementFactory::make(factory).build().map_err(|_| format!("GStreamer has no {factory}"));
    let pipeline = gst::Pipeline::new();
    let src = make("filesrc")?;
    src.set_property("location", input.to_string_lossy().to_string());
    let demux = make("matroskademux")?;
    let mux = make("webmmux")?;
    let sink = make("filesink")?;
    sink.set_property("location", output.to_string_lossy().to_string());
    pipeline.add_many([&src, &demux, &mux, &sink]).map_err(|e| e.to_string())?;
    src.link(&demux).map_err(|e| e.to_string())?;
    mux.link(&sink).map_err(|e| e.to_string())?;

    // Each stream the demuxer finds goes through its own queue into the muxer pad of its kind.
    // The queues are unbounded: MediaRecorder interleaves audio and video loosely, and a bounded
    // queue filled with video while the muxer waited for audio deadlocked the rewrite (seen on a
    // real recording). The muxer pad is asked for by name: a queue's caps aren't known yet, and a
    // "compatible" pad picked from them gave the audio a video pad.
    let pipeline_weak = pipeline.downgrade();
    let mux_weak = mux.downgrade();
    demux.connect_pad_added(move |_, pad| {
        let (Some(pipeline), Some(mux)) = (pipeline_weak.upgrade(), mux_weak.upgrade()) else { return };
        let template = match pad.name().as_str() {
            n if n.starts_with("video") => "video_%u",
            n if n.starts_with("audio") => "audio_%u",
            _ => return, // subtitles and the like: not in a meeting recording
        };
        let Ok(queue) = gst::ElementFactory::make("queue")
            .property("max-size-buffers", 0u32)
            .property("max-size-bytes", 0u32)
            .property("max-size-time", 0u64)
            .build()
        else {
            return;
        };
        if pipeline.add(&queue).is_err() {
            return;
        }
        let _ = queue.sync_state_with_parent();
        let (Some(queue_sink), Some(queue_src)) = (queue.static_pad("sink"), queue.static_pad("src")) else { return };
        let Some(mux_pad) = mux.request_pad_simple(template) else { return };
        if queue_src.link(&mux_pad).is_err() {
            return;
        }
        let _ = pad.link(&queue_sink);
    });

    pipeline.set_state(gst::State::Playing).map_err(|e| format!("couldn't start the rewrite: {e}"))?;
    let bus = pipeline.bus().ok_or("no GStreamer bus")?;
    let outcome = match bus.timed_pop_filtered(gst::ClockTime::from_seconds(TIMEOUT.as_secs()), &[gst::MessageType::Eos, gst::MessageType::Error]) {
        Some(msg) => match msg.view() {
            gst::MessageView::Eos(_) => Ok(()),
            gst::MessageView::Error(err) => Err(format!("couldn't rewrite the recording: {}", err.error())),
            _ => Err("the rewrite stopped unexpectedly".into()),
        },
        None => Err("the rewrite took too long".into()),
    };
    let _ = pipeline.set_state(gst::State::Null);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ZAZABOR_REMUX_FILE=<copy of a recording> cargo test --lib remux_a_real_file -- --ignored`
    #[test]
    #[ignore]
    fn remux_a_real_file() {
        let path = PathBuf::from(std::env::var("ZAZABOR_REMUX_FILE").expect("set ZAZABOR_REMUX_FILE"));
        let started = std::time::Instant::now();
        make_seekable(&path).unwrap();
        eprintln!("rewritten in {:.1} s", started.elapsed().as_secs_f64());
    }

    #[test]
    fn the_part_file_sits_next_to_the_recording() {
        assert_eq!(part_path(Path::new("/v/Meet 2026-11-22.webm")), PathBuf::from("/v/Meet 2026-11-22.webm.seekable-part"));
    }

    /// A short WebM made like the meeting page makes them (no duration), rewritten: needs GStreamer's
    /// vp8enc/opusenc to build the sample, so it skips itself when they're missing.
    #[test]
    fn a_live_webm_gets_a_duration() {
        init().unwrap();
        let dir = std::env::temp_dir().join(format!("zazabor-remux-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("live.webm");
        let launch = format!(
            "videotestsrc num-buffers=60 ! video/x-raw,width=160,height=120,framerate=30/1 ! vp8enc deadline=1 ! webmmux streamable=true ! filesink location={}",
            file.display()
        );
        let Ok(pipeline) = gst::parse_launch(&launch) else { return };
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        bus.timed_pop_filtered(gst::ClockTime::from_seconds(30), &[gst::MessageType::Eos, gst::MessageType::Error]);
        pipeline.set_state(gst::State::Null).unwrap();
        let before = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
        if before == 0 {
            return;
        }
        make_seekable(&file).unwrap();
        let after = std::fs::read(&file).unwrap();
        assert!(!after.is_empty());
        // The Cues element (ID 0x1C53BB6B) is written only by a seekable (non-live) mux.
        assert!(after.windows(4).any(|w| w == [0x1C, 0x53, 0xBB, 0x6B]), "no cues in the rewritten file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
