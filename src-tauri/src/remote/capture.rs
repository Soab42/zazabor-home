//! The PC's screen as low-latency H.264: `ximagesrc` of one monitor → scale → encoder → an
//! `appsink` this module pulls Annex-B access units from (the same appsink pattern as the
//! recorder). The encoder is x264 (`zerolatency`, constant bitrate, which can follow the
//! network) or VA-API (the graphics chip does the work, but this machine's driver only does
//! constant quality, so the bitrate is whatever the picture costs).

use gstreamer as gst;
use gst::prelude::*;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    X264,
    Vaapi,
}

impl Encoder {
    pub fn parse(name: &str) -> Encoder {
        if name == "vaapi" {
            Encoder::Vaapi
        } else {
            Encoder::X264
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Encoder::X264 => "x264",
            Encoder::Vaapi => "vaapi",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Params {
    pub display: String,
    /// The monitor's rectangle on the X screen: x, y, width, height.
    pub rect: (i32, i32, u32, u32),
    /// The picture sent (even sides), scaled from the rectangle.
    pub size: (u32, u32),
    pub fps: u32,
    pub kbps: u32,
    pub encoder: Encoder,
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace(['"', '\\'], ""))
}

/// The pipeline as a `gst-launch` description.
pub fn description(p: &Params) -> String {
    let (x, y, w, h) = p.rect;
    let (ow, oh) = p.size;
    let fps = p.fps.clamp(5, 60);
    let key = fps * 4;
    let encode = match p.encoder {
        Encoder::X264 => format!(
            "videoconvert ! video/x-raw,format=I420 ! x264enc name=enc tune=zerolatency speed-preset=ultrafast bitrate={} key-int-max={key} bframes=0 byte-stream=true sliced-threads=true ! video/x-h264,profile=main",
            p.kbps.max(200)
        ),
        Encoder::Vaapi => format!("videoconvert ! video/x-raw,format=NV12 ! vaapih264enc name=enc rate-control=cqp init-qp=27 keyframe-period={key} max-bframes=0 aud=false"),
    };
    format!(
        "ximagesrc display-name={} use-damage=false show-pointer=true startx={x} starty={y} endx={} endy={} ! video/x-raw,framerate={fps}/1 ! queue max-size-buffers=2 max-size-bytes=0 max-size-time=0 leaky=downstream ! videoconvert ! videoscale ! video/x-raw,width={ow},height={oh},pixel-aspect-ratio=1/1 ! {encode} ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=v sync=false max-buffers=3 drop=true",
        quote(&p.display),
        x + w as i32 - 1,
        y + h as i32 - 1
    )
}

/// A running capture.
pub struct Capture {
    pipeline: gst::Pipeline,
    enc: gst::Element,
    sink: gst::Element,
}

/// One encoded picture.
pub struct Unit {
    pub key: bool,
    pub data: Vec<u8>,
}

impl Capture {
    pub fn start(p: &Params) -> Result<Capture> {
        gst::init().map_err(|e| e.to_string())?;
        let pipeline = gst::parse_launch(&description(p)).map_err(|e| format!("Couldn't set up the screen capture: {e}"))?.downcast::<gst::Pipeline>().map_err(|_| "Couldn't set up the screen capture.".to_string())?;
        let enc = pipeline.by_name("enc").ok_or("The capture has no encoder.")?;
        let sink = pipeline.by_name("v").ok_or("The capture has no output.")?;
        pipeline.set_state(gst::State::Playing).map_err(|e| format!("Couldn't start the screen capture: {e}"))?;
        Ok(Capture { pipeline, enc, sink })
    }

    /// The next picture, waiting up to `wait_ms`; Ok(None) when nothing came yet.
    pub fn pull(&self, wait_ms: u64) -> Result<Option<Unit>> {
        let sample = self.sink.emit_by_name::<Option<gst::Sample>>("try-pull-sample", &[&(wait_ms * gst::ClockTime::MSECOND.nseconds())]);
        let Some(sample) = sample else {
            if let Some(bus) = self.pipeline.bus() {
                if let Some(msg) = bus.pop_filtered(&[gst::MessageType::Error]) {
                    if let gst::MessageView::Error(e) = msg.view() {
                        return Err(format!("The screen capture stopped: {}", e.error()));
                    }
                }
            }
            return Ok(None);
        };
        let Some(buffer) = sample.buffer() else { return Ok(None) };
        let Ok(map) = buffer.map_readable() else { return Ok(None) };
        Ok(Some(Unit { key: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT), data: map.as_slice().to_vec() }))
    }

    /// Asks the encoder for a keyframe with its headers now (a new viewer, or frames were lost).
    pub fn force_key(&self) {
        let event = gst::event::CustomUpstream::new(gst::Structure::builder("GstForceKeyUnit").field("all-headers", true).build());
        self.enc.send_event(event);
    }

    /// Changes the bitrate on the fly (x264 only).
    pub fn set_bitrate(&self, kbps: u32) {
        if self.enc.factory().is_some_and(|f| f.name() == "x264enc") {
            self.enc.set_property("bitrate", kbps.clamp(200, 40_000));
        }
    }

    pub fn stop(self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
