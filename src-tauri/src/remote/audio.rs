//! What the phone hears of the PC during a session: the PC's own sound (what its speakers play)
//! or its microphone, as Opus (48 kHz stereo, 20 ms packets) pulled from an `appsink`. The phone
//! decodes it with MediaCodec and plays it as it comes.

use gstreamer as gst;
use gst::prelude::*;

type Result<T> = std::result::Result<T, String>;

/// Which sound goes to the phone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Audio {
    Off,
    /// What the PC's speakers play.
    Sound,
    /// The PC's default microphone.
    Mic,
}

impl Audio {
    pub fn parse(s: &str) -> Audio {
        match s {
            "sound" => Audio::Sound,
            "mic" => Audio::Mic,
            _ => Audio::Off,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Audio::Off => "off",
            Audio::Sound => "sound",
            Audio::Mic => "mic",
        }
    }
}

pub const RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
/// The samples the decoder drops at the start (opusenc's look-ahead at 48 kHz).
pub const PRE_SKIP: u32 = 312;

/// The pipeline as a `gst-launch` description; None for `Off`.
pub fn description(which: Audio, kbps: u32) -> Option<String> {
    let source = match which {
        Audio::Off => return None,
        Audio::Sound => "pulsesrc device=\"@DEFAULT_MONITOR@\"",
        Audio::Mic => "pulsesrc device=\"@DEFAULT_SOURCE@\"",
    };
    Some(format!(
        "{source} buffer-time=100000 latency-time=20000 ! queue max-size-buffers=0 max-size-bytes=0 max-size-time=200000000 leaky=downstream ! audioconvert ! audioresample ! audio/x-raw,rate={RATE},channels={CHANNELS} ! opusenc bitrate={} frame-size=20 ! appsink name=a sync=false max-buffers=50 drop=true",
        kbps.clamp(16, 256) * 1000
    ))
}

pub struct Capture {
    pipeline: gst::Pipeline,
    sink: gst::Element,
}

impl Capture {
    pub fn start(which: Audio, kbps: u32) -> Result<Capture> {
        let text = description(which, kbps).ok_or("No sound was asked for.")?;
        gst::init().map_err(|e| e.to_string())?;
        let pipeline = gst::parse_launch(&text).map_err(|e| format!("Couldn't open the PC's sound: {e}"))?.downcast::<gst::Pipeline>().map_err(|_| "Couldn't open the PC's sound.".to_string())?;
        let sink = pipeline.by_name("a").ok_or("The sound has no output.")?;
        pipeline.set_state(gst::State::Playing).map_err(|e| format!("Couldn't start the PC's sound: {e}"))?;
        Ok(Capture { pipeline, sink })
    }

    /// The next Opus packet, waiting up to `wait_ms`.
    pub fn pull(&self, wait_ms: u64) -> Result<Option<Vec<u8>>> {
        let sample = self.sink.emit_by_name::<Option<gst::Sample>>("try-pull-sample", &[&(wait_ms * gst::ClockTime::MSECOND.nseconds())]);
        let Some(sample) = sample else {
            if let Some(bus) = self.pipeline.bus() {
                if let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
                    if let gst::MessageView::Error(e) = msg.view() {
                        return Err(format!("The PC's sound stopped: {}", e.error()));
                    }
                }
            }
            return Ok(None);
        };
        let Some(buffer) = sample.buffer() else { return Ok(None) };
        let Ok(map) = buffer.map_readable() else { return Ok(None) };
        Ok(Some(map.as_slice().to_vec()))
    }

    /// Stops it (also what dropping it does).
    pub fn stop(self) {}
}

impl Drop for Capture {
    /// Tearing a pipeline down can block (a camera that doesn't let go): that happens on a
    /// thread of its own, so the session never waits on it.
    fn drop(&mut self) {
        let pipeline = self.pipeline.clone();
        let _ = std::thread::Builder::new().name("remote-gst-stop".into()).spawn(move || {
            let _ = pipeline.set_state(gst::State::Null);
        });
    }
}
