//! Audio output for local playback.
//!
//! librespot's rodio sink panics if no output device is available. Release
//! builds abort on that panic. This sink opens the device when playback starts
//! and reports a device it cannot open through the UI, from the first write
//! (see `start`). LWSpot can then remain available as a Connect remote
//! until an output appears.
//!
//! fastframe-audio owns the device stream: it pauses with playback, so a
//! paused player costs no audio work (#636), and reopens after a failure.
//! rodio's mixer and queue fill it. The system's default output is opened
//! as the endpoint it is at that moment, and followed by this sink (see
//! `endpoint` and `Follow`).

#[cfg(windows)]
mod endpoint;

/// Off Windows the sink opens "the default" and fastframe-audio follows it
/// through the system's own default-device notifications (CoreAudio's on
/// macOS), so there is no endpoint to read and no polling thread.
#[cfg(not(windows))]
#[allow(dead_code)]
mod endpoint {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct Endpoint {
        pub(super) id: String,
        pub(super) name: String,
    }

    pub(super) struct Apartment;

    impl Apartment {
        pub(super) fn enter() -> Result<Self, String> {
            Ok(Self)
        }
    }

    pub(super) fn default_output() -> Result<Option<Endpoint>, String> {
        Err("the default output is followed by the audio stream".into())
    }
}

use std::borrow::Cow;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError, TryLockError, Weak};
use std::thread;
use std::time::{Duration, Instant};

use fastframe_audio::{Buffer, BufferSize, Maintained, OutputOptions, Render};
use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::mixer::VolumeGetter;
use librespot_playback::player::PlayerEvent;
use librespot_playback::{NUM_CHANNELS, SAMPLE_RATE};
use rodio::Source;

use crate::i18n::{Locale, gettext};
use crate::resample::Resampler;
use endpoint::Endpoint;

/// The backend name Settings uses for this sink.
pub const NAME: &str = "rodio";

/// Told about output failures, with a message fit for the interface.
pub type ErrorHook = Arc<dyn Fn(String) + Send + Sync>;

/// Reported when the system has no audio output at all. The interface
/// recognises it and shows it in the user's language.
pub const NO_DEVICE: &str =
    "No audio output device was found. Connect or enable one, then press play again.";

/// Opens the output: the device by name, else the default.
type Opener = fn(Option<&str>, u32, &AudioControl) -> Result<Output, OpenError>;

/// Maximum queued rodio chunks before `write` blocks, about 200 ms of audio.
const QUEUE_LIMIT: usize = 12;

/// How much of a full queue rodio plays before `write` is woken to top it
/// up. librespot's packets hold 4 to 13 ms of sound, so polling every 10 ms
/// (or waking for every packet) kept the decoder thread awake about 100
/// times a second; this wakes it about 30 times.
const REFILL: Duration = Duration::from_millis(20);

/// Longest `write` waits for rodio before it looks at the device again: a
/// stream that has failed finishes no chunk to wake it.
const QUEUE_WAIT: Duration = Duration::from_millis(50);

/// How long a full queue may go without rodio finishing a chunk before the
/// output counts as stuck. A few times the longest device buffer.
const STALL: Duration = Duration::from_secs(2);

/// Watches a waiting writer's queue for an output that stopped playing.
struct Stall {
    consumed: u64,
    since: Instant,
    seen: Instant,
}

impl Stall {
    fn new(consumed: u64, now: Instant) -> Self {
        Self {
            consumed,
            since: now,
            seen: now,
        }
    }

    /// Notes how far rodio has got; true once it has not moved for `STALL`.
    fn observe(&mut self, consumed: u64, now: Instant) -> bool {
        // No wait lasts anywhere near `STALL`, so a gap that long is time
        // this thread did not run: the system slept or suspended the app,
        // and the output had no chance to play either. On Windows `Instant`
        // counts that time.
        let asleep = now.duration_since(self.seen) >= STALL;
        self.seen = now;
        if consumed != self.consumed || asleep {
            *self = Self::new(consumed, now);
            return false;
        }
        now.duration_since(self.since) >= STALL
    }
}

/// Maximum time `stop` waits for the queue to drain.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Length of each side of an interrupted-track fade.
const INTERRUPT_FADE: Duration = Duration::from_millis(50);

/// How long the queue after a confirmed seek takes to come up. The old
/// position is already gone, so this only has to keep the cut into the new
/// one from clicking (#343).
const SEEK_FADE: Duration = Duration::from_millis(8);

/// Attempts at opening the output before a failure is reported. A device
/// that is changing hands (headphones that connect, a new default output)
/// can refuse for a moment.
const OPEN_ATTEMPTS: u32 = 5;

/// The wait between those attempts.
const OPEN_RETRY: Duration = Duration::from_millis(150);

/// How often the default output is checked while the sink follows it.
#[cfg_attr(not(windows), allow(dead_code))]
const DEFAULT_POLL: Duration = Duration::from_millis(1_500);

/// The longest a seek command is remembered while waiting for librespot to
/// confirm it, for the timing in the log.
const SEEK_TIMING: Duration = Duration::from_secs(10);

/// How long Play takes to come up, and Pause and Stop to go down.
const TRANSPORT_FADE: Duration = Duration::from_millis(50);

/// Smooths slider and mute changes at the device's sample rate.
const VOLUME_RAMP: Duration = Duration::from_millis(30);

/// Default Windows device buffer length in milliseconds.
///
/// Small platform defaults can click under load (#88). A 100 ms buffer avoids
/// these underruns while keeping controls responsive.
pub const DEFAULT_BUFFER_MS: u32 = 100;

/// Allowed Windows device buffer range. Lower values can click; higher values
/// delay playback controls.
pub const BUFFER_MS_RANGE: std::ops::RangeInclusive<u32> = 20..=500;

/// Coordinates an explicit track replacement with the audio thread.
///
/// librespot deliberately leaves a gapless sink running between tracks. That
/// is right when one track reaches its end, but an explicit skip otherwise
/// leaves the old queued audio in front of the replacement. The old signal is
/// faded on rodio's output thread before its queue is discarded; writes stay
/// gated until librespot reports that the replacement track is loaded.
/// A confirmed seek also discards queued audio, without gating packets from
/// the decoder that has already moved to the requested position.
pub struct AudioControl {
    target: Mutex<AudioTarget>,
    waiting_for_track: AtomicBool,
    /// The pending `Reset`, as its number; zero for none.
    reset_output: AtomicU8,
    reset_processing: AtomicBool,
    buffer_ms: u32,
    seek: Mutex<SeekTiming>,
}

#[derive(Default)]
struct AudioTarget {
    sink: Weak<rodio::Sink>,
    envelope: Option<Arc<Envelope>>,
}

/// Why the output queue is being replaced, which sets how fast the new one
/// comes up. Numbered so the stronger of two pending reasons wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reset {
    /// librespot confirmed a seek: the old position stopped at once.
    Seek = 1,
    /// The listener replaced the track, which faded out first.
    Interrupt = 2,
}

impl Reset {
    fn from_number(number: u8) -> Option<Self> {
        match number {
            1 => Some(Self::Seek),
            2 => Some(Self::Interrupt),
            _ => None,
        }
    }

    /// How long the replacement queue takes to come up.
    fn fade_in(self) -> Duration {
        match self {
            Self::Seek => SEEK_FADE,
            Self::Interrupt => INTERRUPT_FADE,
        }
    }
}

/// When the last seek was asked for and confirmed, for the debug log.
#[derive(Default)]
struct SeekTiming {
    requested: Option<Instant>,
    confirmed: Option<Instant>,
}

/// Milliseconds from `from` to `to`, for timing lines in the log.
fn millis(from: Instant, to: Instant) -> u128 {
    to.saturating_duration_since(from).as_millis()
}

impl AudioControl {
    pub fn new(buffer_ms: u32) -> Arc<Self> {
        Arc::new(Self {
            target: Mutex::new(AudioTarget::default()),
            waiting_for_track: AtomicBool::new(false),
            reset_output: AtomicU8::new(0),
            reset_processing: AtomicBool::new(false),
            buffer_ms: buffer_ms.clamp(*BUFFER_MS_RANGE.start(), *BUFFER_MS_RANGE.end()),
            seek: Mutex::new(SeekTiming::default()),
        })
    }

    /// Notes that this app asked librespot to seek, so the debug log can
    /// time the seek through to the first sound from the new position.
    pub fn seek_requested(&self) {
        *self.seek.lock().unwrap_or_else(PoisonError::into_inner) = SeekTiming {
            requested: Some(Instant::now()),
            confirmed: None,
        };
    }

    fn seek_confirmed(&self) {
        let now = Instant::now();
        let mut seek = self.seek.lock().unwrap_or_else(PoisonError::into_inner);
        seek.requested = seek
            .requested
            .filter(|requested| now.saturating_duration_since(*requested) < SEEK_TIMING);
        match seek.requested {
            Some(requested) => log::debug!(
                "seek: confirmed by librespot {} ms after the command",
                millis(requested, now)
            ),
            None => log::debug!("seek: confirmed by librespot (asked for elsewhere)"),
        }
        seek.confirmed = Some(now);
    }

    /// The first packet from the new position has been queued.
    fn seek_queued(&self) {
        let now = Instant::now();
        let seek = std::mem::take(&mut *self.seek.lock().unwrap_or_else(PoisonError::into_inner));
        if let Some(confirmed) = seek.confirmed {
            let command = seek
                .requested
                .map(|requested| format!(", {} ms after the command", millis(requested, now)))
                .unwrap_or_default();
            log::debug!(
                "seek: first audio queued {} ms after the confirmation{command}",
                millis(confirmed, now)
            );
        }
    }

    /// Follows confirmed decoder transitions, including seeks requested by
    /// another Spotify client. Natural track changes retain gapless audio.
    pub(crate) fn handle_player_event(&self, event: &PlayerEvent) {
        match event {
            PlayerEvent::TrackChanged { .. } => self.track_changed(),
            PlayerEvent::Seeked { .. } => {
                self.seek_confirmed();
                let target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(sink) = target.sink.upgrade() {
                    sink.stop();
                }
                self.reset_output
                    .fetch_max(Reset::Seek as u8, Ordering::SeqCst);
                self.reset_processing.store(true, Ordering::SeqCst);
                // Previous can rewind the current track after interrupting
                // it. Release that gate, but never close it for a seek:
                // the decoder is already sending audio from the new position.
                self.track_changed();
            }
            PlayerEvent::Stopped { .. } => self.stopped(),
            _ => {}
        }
    }

    /// Fades and discards the current output before a user-requested track
    /// change. Repeated skips share the same handoff.
    pub fn interrupt(&self) {
        if self.waiting_for_track.swap(true, Ordering::SeqCst) {
            return;
        }
        let (sink, envelope) = {
            let target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
            (target.sink.upgrade(), target.envelope.clone())
        };
        if let (Some(sink), Some(envelope)) = (&sink, &envelope) {
            envelope.fade_out();
            let wait =
                Duration::from_millis(u64::from(self.buffer_ms)).saturating_add(INTERRUPT_FADE * 2);
            let deadline = Instant::now() + wait;
            while !envelope.silent() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(1));
            }
            // Unlike `clear`, this does not wait for every queued source.
            // The replacement gets a fresh rodio sink on its first write.
            sink.stop();
        }
        self.reset_output
            .fetch_max(Reset::Interrupt as u8, Ordering::SeqCst);
        self.reset_processing.store(true, Ordering::SeqCst);
    }

    /// Opens the write gate once librespot has left the old decoder behind.
    pub fn track_changed(&self) {
        self.waiting_for_track.store(false, Ordering::SeqCst);
    }

    /// Releases the gate if the requested replacement stopped instead.
    pub fn stopped(&self) {
        self.waiting_for_track.store(false, Ordering::SeqCst);
    }

    fn waiting_for_track(&self) -> bool {
        self.waiting_for_track.load(Ordering::SeqCst)
    }

    /// The processing wrapper owns a separate reset from the output queue.
    /// Keep it pending while old decoder packets are still being discarded.
    pub(crate) fn take_processing_reset(&self) -> bool {
        !self.waiting_for_track() && self.reset_processing.swap(false, Ordering::SeqCst)
    }

    fn take_reset(&self) -> Option<Reset> {
        Reset::from_number(self.reset_output.swap(0, Ordering::SeqCst))
    }

    fn register(&self, sink: &Arc<rodio::Sink>, envelope: Arc<Envelope>) {
        let mut target = self.target.lock().unwrap_or_else(PoisonError::into_inner);
        target.sink = Arc::downgrade(sink);
        target.envelope = Some(envelope);
    }
}

/// Frames handed to rodio, and frames it has finished with.
/// The difference is what is still queued.
struct Queued {
    appended: AtomicU64,
    consumed: AtomicU64,
    /// While `write` waits for room, the queued level at or below which it
    /// is woken; zero while nothing waits.
    wake_at: AtomicU64,
}

impl Queued {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            appended: AtomicU64::new(0),
            consumed: AtomicU64::new(0),
            wake_at: AtomicU64::new(0),
        })
    }

    /// Frames handed over and not yet played.
    fn frames(&self) -> u64 {
        self.appended
            .load(Ordering::Relaxed)
            .saturating_sub(self.consumed.load(Ordering::Relaxed))
    }

    /// The frames rodio has finished with so far.
    fn consumed(&self) -> u64 {
        self.consumed.load(Ordering::Relaxed)
    }

    /// Sleeps the writer until rodio has played `refill` frames of what is
    /// queued, or `QUEUE_WAIT` has passed.
    fn wait_for_room(&self, refill: u64) {
        let wake_at = self.frames().saturating_sub(refill).max(1);
        self.wake_at.store(wake_at, Ordering::Relaxed);
        // Pairs with the fence in `drained`: either the chunk that reaches
        // the level sees it, or this sees that chunk already gone.
        fence(Ordering::SeqCst);
        if self.frames() > wake_at {
            thread::park_timeout(QUEUE_WAIT);
        }
        self.wake_at.store(0, Ordering::Relaxed);
    }

    /// rodio has finished with a chunk: wakes `writer` once the queue has
    /// drained to the level it waits for, and only then.
    fn drained(&self, writer: &thread::Thread) {
        fence(Ordering::SeqCst);
        let wake_at = self.wake_at.load(Ordering::Relaxed);
        if wake_at != 0
            && self.frames() <= wake_at
            && self
                .wake_at
                .compare_exchange(wake_at, 0, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            writer.unpark();
        }
    }
}

/// The range a level moves over, as a fixed point fraction of full gain.
const SCALE: u32 = 1 << 24;

/// A sample-clocked gain shared by every chunk in one rodio queue.
struct Envelope {
    level: AtomicU32,
    target: AtomicU32,
    /// How far `level` moves each frame. Set per fade, so a ramp can be cut
    /// to fit the sound that is left to carry it.
    step: AtomicU32,
    /// The step for this envelope's nominal length, and its slowest.
    full_step: u32,
}

impl Envelope {
    /// Resting fully open, for a signal that is already sounding.
    fn open(sample_rate: u32, length: Duration) -> Arc<Self> {
        Self::at(sample_rate, length, SCALE)
    }

    /// Resting closed, and staying there until something raises it.
    fn closed(sample_rate: u32, length: Duration) -> Arc<Self> {
        Self::at(sample_rate, length, 0)
    }

    /// Closed, and already on its way up over `rise`. Later fades take the
    /// nominal `length`.
    fn rising(sample_rate: u32, length: Duration, rise: Duration) -> Arc<Self> {
        let envelope = Self::closed(sample_rate, length);
        envelope
            .step
            .store(step_over(fade_frames(sample_rate, rise)), Ordering::Relaxed);
        envelope.target.store(SCALE, Ordering::Relaxed);
        envelope
    }

    fn at(sample_rate: u32, length: Duration, level: u32) -> Arc<Self> {
        let full_step = step_over(fade_frames(sample_rate, length));
        Arc::new(Self {
            level: AtomicU32::new(level),
            target: AtomicU32::new(level),
            step: AtomicU32::new(full_step),
            full_step,
        })
    }

    fn fade_in(&self) {
        self.step.store(self.full_step, Ordering::Relaxed);
        self.target.store(SCALE, Ordering::Relaxed);
    }

    fn fade_out(&self) {
        self.step.store(self.full_step, Ordering::Relaxed);
        self.target.store(0, Ordering::Relaxed);
    }

    /// Fades out over `frames` of sound, or the nominal length if that is
    /// shorter.
    fn fade_out_over(&self, frames: u64) {
        let frames = frames.clamp(1, u64::from(u32::MAX)) as u32;
        self.step
            .store(step_over(frames).max(self.full_step), Ordering::Relaxed);
        self.target.store(0, Ordering::Relaxed);
    }

    /// Puts the envelope at silence at once, wherever its ramp had reached.
    /// Callers use this once the sound has stopped and there is no longer
    /// anything for a ramp to ride.
    fn close(&self) {
        self.step.store(self.full_step, Ordering::Relaxed);
        self.target.store(0, Ordering::Relaxed);
        self.level.store(0, Ordering::Relaxed);
    }

    fn silent(&self) -> bool {
        self.level.load(Ordering::Relaxed) == 0
    }

    /// Returns this frame's gain, then moves one frame toward the target.
    fn next_gain(&self) -> f32 {
        let level = self.level.load(Ordering::Relaxed);
        let target = self.target.load(Ordering::Relaxed);
        let step = self.step.load(Ordering::Relaxed);
        let next = match level.cmp(&target) {
            std::cmp::Ordering::Less => level.saturating_add(step).min(target),
            std::cmp::Ordering::Greater => level.saturating_sub(step).max(target),
            std::cmp::Ordering::Equal => level,
        };
        self.level.store(next, Ordering::Relaxed);
        level as f32 / SCALE as f32
    }
}

/// The per-frame movement that crosses the whole range in `frames`.
fn step_over(frames: u32) -> u32 {
    SCALE.div_ceil(frames.max(1)).max(1)
}

fn fade_frames(sample_rate: u32, length: Duration) -> u32 {
    (u64::from(sample_rate) * length.as_millis() as u64 / 1_000).max(1) as u32
}

/// Applies the shared interruption envelope on rodio's output thread, so it
/// can smooth audio that was already queued when the user changes track.
struct TransitionSource {
    inner: rodio::buffer::SamplesBuffer,
    /// Smooths a track the listener replaced part way through.
    interrupt: Arc<Envelope>,
    /// Carries Play and Pause.
    transport: Arc<Envelope>,
    /// The count this chunk's frames belong to.
    queued: Arc<Queued>,
    /// Frames of this chunk not yet handed on.
    remaining: u32,
    channel: usize,
    /// Samples in each of this chunk's frames.
    channels: usize,
    gain: f32,
    /// The thread that queued this chunk, which may be waiting in `write`
    /// for room.
    writer: thread::Thread,
}

impl TransitionSource {
    fn new(
        inner: rodio::buffer::SamplesBuffer,
        interrupt: Arc<Envelope>,
        transport: Arc<Envelope>,
        queued: Arc<Queued>,
        frames: u32,
    ) -> Self {
        let channels = usize::from(inner.channels()).max(1);
        Self {
            inner,
            interrupt,
            transport,
            queued,
            remaining: frames,
            channel: 0,
            channels,
            gain: 1.0,
            writer: thread::current(),
        }
    }
}

impl Drop for TransitionSource {
    /// rodio drops whole sources on `stop`, which every track change does, so
    /// a chunk can end without being played. Settling up here is what stops
    /// the count drifting away from the queue it is meant to describe.
    ///
    /// rodio drops a chunk once it has taken it out of the sink's count, so
    /// this is also where a writer waiting for room learns of it.
    fn drop(&mut self) {
        self.queued
            .consumed
            .fetch_add(u64::from(self.remaining), Ordering::Relaxed);
        self.queued.drained(&self.writer);
    }
}

impl Iterator for TransitionSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.channel == 0 {
            // Both step every frame. They are independent ramps that happen
            // to share a signal, so a skip during a pause rides them at once.
            self.gain = self.interrupt.next_gain() * self.transport.next_gain();
            self.remaining = self.remaining.saturating_sub(1);
            self.queued.consumed.fetch_add(1, Ordering::Relaxed);
        }
        self.channel = (self.channel + 1) % self.channels;
        Some(sample * self.gain)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl Source for TransitionSource {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

pub struct RodioSink {
    /// The output device name from Settings; `None` means the default.
    device: Option<String>,
    output: Option<Output>,
    on_error: ErrorHook,
    /// Player volume, applied at output so changes affect queued audio.
    volume: Box<dyn VolumeGetter + Send>,
    applied_volume: f32,
    /// How much sound to ask the device to hold, in milliseconds. Taken
    /// when the stream opens, so a change lands with the next restart.
    buffer_ms: u32,
    control: Arc<AudioControl>,
    open: Opener,
}

struct Output {
    device: fastframe_audio::Output<MixerRender>,
    volume: Arc<AtomicU32>,
    /// What every renderer made for this output plays from, so a stream
    /// opened on another device carries on from the same sample.
    mix: Arc<Mutex<Mix>>,
    /// Where a mixer made for a new stream format waits for this thread.
    made: MixerSlot,
    mixer: rodio::mixer::Mixer,
    sink: Arc<rodio::Sink>,
    /// The rate the mixer runs at, and the converter to it when that is
    /// not Spotify's.
    sample_rate: u32,
    /// The mixer's channel count.
    channels: u16,
    resampler: Option<Resampler>,
    envelope: Arc<Envelope>,
    /// The Play and Pause ramp, kept across track changes so a skip during a
    /// fade does not snap the level back.
    transport: Arc<Envelope>,
    /// How much sound is queued, so Pause can cut its ramp to fit.
    queued: Arc<Queued>,
    /// Whether this track has supplied audio since its last stop.
    fed: bool,
    last_write: Option<Instant>,
    /// For a stream on the system's default output, the endpoint it is on
    /// and what the default is now.
    follow: Option<Follow>,
    buffer_ms: u32,
}

impl Output {
    fn failed(&self) -> bool {
        self.device.failed()
    }

    /// Plays from `mixer`, made for the format the device now runs at, with
    /// a fresh queue and ramps measured at its rate.
    fn attach(&mut self, (mixer, sample_rate, channels): MadeMixer, control: &AudioControl) {
        let sink = Arc::new(rodio::Sink::connect_new(&mixer));
        let envelope = Envelope::open(sample_rate, INTERRUPT_FADE);
        control.register(&sink, Arc::clone(&envelope));
        self.resampler = converter_to(sample_rate);
        self.mixer = mixer;
        self.sink = sink;
        self.sample_rate = sample_rate;
        self.channels = channels;
        self.envelope = envelope;
        // The first sound has silence to come up from instead of a hard edge.
        self.transport = Envelope::closed(sample_rate, TRANSPORT_FADE);
        self.queued = Queued::new();
        self.fed = false;
        self.last_write = None;
    }

    /// Has the device ask for sound, reopening it if it failed, moved to a
    /// new default output, or was let go after a long pause. Returns whether
    /// the queue was replaced, which needs the volume set again.
    fn run(&mut self, control: &AudioControl) -> Result<bool, OpenError> {
        self.follow_default()?;
        self.device.resume();
        for error in self.device.take_errors() {
            if error.is_fatal() {
                log::error!("audio stream error: {error}");
            } else {
                log::warn!("audio stream error: {error}");
            }
        }
        match self.device.maintain() {
            Maintained::Reopened {
                device,
                sample_rate,
                channels,
                reason,
            } => log_stream(
                &format!("reopened ({reason:?})"),
                &device,
                sample_rate,
                channels,
            ),
            Maintained::Failed(error) => return Err(error.into()),
            Maintained::Unchanged | Maintained::Released => {}
        }
        if self.device.failed() {
            return Err(OpenError::Stopped);
        }
        let made = self
            .made
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(mixer) = made else {
            return Ok(false);
        };
        self.attach(mixer, control);
        Ok(true)
    }

    /// Moves a stream on the default output to the endpoint that is the
    /// default now. A stream that failed asks afresh, because the device
    /// that went away may have taken the default with it, and reopening it
    /// by name would find nothing.
    fn follow_default(&mut self) -> Result<(), OpenError> {
        let lost = self.device.failed();
        let Some(follow) = &mut self.follow else {
            return Ok(());
        };
        let target = if lost {
            match endpoint::default_output() {
                Ok(Some(now)) => {
                    follow.noted(&now);
                    (now != follow.opened).then_some(now)
                }
                Ok(None) => return Err(OpenError::NoDevice),
                Err(error) => {
                    log::debug!("cannot check the default audio output: {error}");
                    None
                }
            }
        } else {
            follow.moved()
        };
        match target {
            Some(target) => self.move_to(target, lost),
            None => Ok(()),
        }
    }

    /// Opens `target` with a renderer that carries on from this output's
    /// mixer, and lets the old stream go. A stream that still plays stays
    /// where it is if the new one will not open, and that endpoint is not
    /// tried again until the default moves on.
    fn move_to(&mut self, target: Endpoint, lost: bool) -> Result<(), OpenError> {
        let Some(follow) = self.follow.as_mut() else {
            return Ok(());
        };
        // The old stream stops asking for sound first, so the two never take
        // turns at the mixer.
        self.device.pause();
        let renderer = MixerRender::new(&self.volume, &self.mix, &self.made);
        let opened = open_device(
            fastframe_audio::Device::Named(target.name.clone()),
            self.buffer_ms,
            renderer,
        );
        match opened {
            Ok(device) => {
                log_stream(
                    "moved to the default output",
                    device.device_name(),
                    device.sample_rate(),
                    device.channels(),
                );
                self.device = device;
                follow.opened = target;
                follow.declined = None;
                Ok(())
            }
            Err(error) if lost => Err(error.into()),
            Err(error) => {
                log::warn!(
                    "cannot move to the default audio output {:?}: {error}; staying on {:?}",
                    target.name,
                    follow.opened.name
                );
                follow.declined = Some(target.id);
                self.device.resume();
                Ok(())
            }
        }
    }
}

/// Follows the system's default output for a stream opened on it as the
/// endpoint it was. cpal would follow it too, through the path that broke
/// reopening (see `endpoint`), so the sink polls instead.
#[cfg_attr(not(windows), allow(dead_code))]
struct Follow {
    /// The endpoint the stream is open on.
    opened: Endpoint,
    /// The default as last seen, kept current by a thread that ends with
    /// this.
    latest: Arc<Mutex<Option<Endpoint>>>,
    /// The id of a default that would not open.
    declined: Option<String>,
}

impl Follow {
    #[cfg_attr(not(windows), allow(dead_code))]
    fn start(opened: Endpoint) -> Self {
        let latest = Arc::new(Mutex::new(Some(opened.clone())));
        let watched = Arc::downgrade(&latest);
        let started = thread::Builder::new()
            .name("audio-default-follow".into())
            .spawn(move || {
                // COM for this thread's life, in the apartment the device
                // enumerator is made for.
                let _apartment = endpoint::Apartment::enter();
                loop {
                    thread::sleep(DEFAULT_POLL);
                    let Some(latest) = watched.upgrade() else {
                        break;
                    };
                    match endpoint::default_output() {
                        Ok(now) => *latest.lock().unwrap_or_else(PoisonError::into_inner) = now,
                        Err(error) => log::debug!("cannot check the default audio output: {error}"),
                    }
                }
            });
        if let Err(error) = started {
            log::warn!("cannot follow the default audio output: {error}");
        }
        Self {
            opened,
            latest,
            declined: None,
        }
    }

    /// Records a default read outside the polling thread.
    fn noted(&self, now: &Endpoint) {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = Some(now.clone());
    }

    /// The endpoint to move to, when the default is no longer the one the
    /// stream is on.
    /// Asked before every write, so nothing is copied unless it moved.
    fn moved(&mut self) -> Option<Endpoint> {
        let latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        if latest.as_ref() == Some(&self.opened) {
            // Back where it started: a declined endpoint may be tried again
            // when it next becomes the default.
            self.declined = None;
            return None;
        }
        latest
            .as_ref()
            .filter(|now| default_moved(&self.opened, Some(now), self.declined.as_deref()))
            .cloned()
    }
}

/// Whether a stream open on `opened` should move to `now`, the current
/// default output. Without a default it stays: a stream with nowhere to go
/// is the failure path's to handle. A default that would not open is not
/// tried again while it stays the default.
fn default_moved(opened: &Endpoint, now: Option<&Endpoint>, declined: Option<&str>) -> bool {
    now.is_some_and(|now| now != opened && declined != Some(now.id.as_str()))
}

/// Logs a stream the sink opened, as a bug report needs it: which device,
/// and the rate and channels it plays at (#804).
fn log_stream(event: &str, device: &str, sample_rate: u32, channels: u16) {
    log::info!("audio output {event}: {device} at {sample_rate} Hz, {channels} channels");
    if let Some(note) = channel_note(channels) {
        log::warn!("audio output {device}: {note}");
    }
}

/// What a stream that is not stereo does to Spotify's stereo.
fn channel_note(channels: u16) -> Option<&'static str> {
    match channels {
        2 => None,
        1 => Some("the device takes one channel; both sides of the music are mixed into it"),
        _ => Some("the device takes more than two channels; the music plays on the first two"),
    }
}

/// The samples and channel count to queue for a mixer with `channels`.
///
/// rodio turns stereo into mono by dropping the right channel, so a mono
/// output would lose half of every hard-panned recording. It gets both
/// sides mixed instead. Wider outputs take stereo as it is: rodio puts it
/// on the first two channels, the front left and right.
fn fit_channels(samples: Vec<f32>, channels: u16) -> (u16, Vec<f32>) {
    if channels != 1 {
        return (NUM_CHANNELS.into(), samples);
    }
    let mono = samples
        .as_chunks::<{ NUM_CHANNELS as usize }>()
        .0
        .iter()
        .map(|[left, right]| (left + right) / 2.0)
        .collect();
    (1, mono)
}

/// The converter from Spotify's rate to `sample_rate`, when they differ.
fn converter_to(sample_rate: u32) -> Option<Resampler> {
    let resampler = Resampler::new(SAMPLE_RATE, sample_rate, NUM_CHANNELS as usize);
    if resampler.is_some() {
        log::info!(
            "the output runs at {sample_rate} Hz; the music is converted from {SAMPLE_RATE} Hz"
        );
    }
    resampler
}

/// A mixer, the rate it runs at and its channel count.
type MadeMixer = (rodio::mixer::Mixer, u32, u16);

type MixerSlot = Arc<Mutex<Option<MadeMixer>>>;

/// A linear gain ramp, shared by every channel of an output frame.
#[derive(Default)]
struct VolumeRamp {
    current: f32,
    target: f32,
    remaining: u32,
    sample_rate: u32,
}

impl VolumeRamp {
    fn next_gain(&mut self, target: f32, sample_rate: u32) -> f32 {
        if target != self.target || sample_rate != self.sample_rate {
            self.target = target;
            self.sample_rate = sample_rate;
            self.remaining = fade_frames(sample_rate, VOLUME_RAMP);
        }
        let gain = self.current;
        if self.remaining > 0 {
            self.current += (self.target - self.current) / self.remaining as f32;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            }
        }
        gain
    }
}

/// The mixer's output and the format it was made for, shared by every
/// renderer an output makes, so a stream on another device plays on from
/// it.
#[derive(Default)]
struct Mix {
    source: Option<rodio::mixer::MixerSource>,
    format: (u32, u16),
}

/// Fills the device from rodio's mixer, or with silence before there is one.
///
/// fastframe-audio configures it on the sink's thread whenever it opens a
/// stream. A stream in the format the mixer already has keeps it, so a
/// reopen on another device carries on from the same sample; another rate or
/// channel count gets a new mixer, which waits in `made` for the sink.
struct MixerRender {
    volume: Arc<AtomicU32>,
    ramp: VolumeRamp,
    mix: Arc<Mutex<Mix>>,
    made: MixerSlot,
}

impl MixerRender {
    /// A renderer for a new stream, coming up from silence to the volume.
    fn new(volume: &Arc<AtomicU32>, mix: &Arc<Mutex<Mix>>, made: &MixerSlot) -> Self {
        Self {
            volume: Arc::clone(volume),
            ramp: VolumeRamp::default(),
            mix: Arc::clone(mix),
            made: Arc::clone(made),
        }
    }
}

impl Render for MixerRender {
    fn configure(&mut self, sample_rate: u32, channels: u16) {
        let mut mix = self.mix.lock().unwrap_or_else(PoisonError::into_inner);
        if mix.source.is_some() && mix.format == (sample_rate, channels) {
            return;
        }
        let (mixer, source) = rodio::mixer::mixer(
            channels as rodio::ChannelCount,
            sample_rate as rodio::SampleRate,
        );
        mix.source = Some(source);
        mix.format = (sample_rate, channels);
        *self.made.lock().unwrap_or_else(PoisonError::into_inner) =
            Some((mixer, sample_rate, channels));
    }

    fn render(&mut self, out: &mut [f32]) {
        // Only a stream being replaced shares the mix, for a callback or
        // two; that callback plays silence rather than wait.
        let mut mix = match self.mix.try_lock() {
            Ok(mix) => mix,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                out.fill(0.0);
                return;
            }
        };
        let (sample_rate, channels) = mix.format;
        match &mut mix.source {
            Some(source) => {
                let target = f32::from_bits(self.volume.load(Ordering::Relaxed));
                for frame in out.chunks_mut(usize::from(channels).max(1)) {
                    let gain = self.ramp.next_gain(target, sample_rate);
                    for sample in frame {
                        *sample = source.next().unwrap_or(0.0) * gain;
                    }
                }
            }
            None => out.fill(0.0),
        }
    }
}

/// Whether to try opening the output again after `failures` failed
/// attempts, and after how long.
fn retry_delay(failures: u32, retryable: bool) -> Option<Duration> {
    (retryable && failures < OPEN_ATTEMPTS).then_some(OPEN_RETRY)
}

/// Whether an open that failed this way can work a moment later.
fn retryable(error: &OpenError) -> bool {
    match error {
        // A default output that is changing hands can be missing, or refuse,
        // for a moment.
        OpenError::NoDevice | OpenError::Stopped => true,
        OpenError::Device(fastframe_audio::OpenError::NoDevice) => true,
        // Denied access, an unsupported format or no audio service at all
        // will not change in a second.
        OpenError::Device(fastframe_audio::OpenError::Device(error)) => !error.is_fatal(),
    }
}

/// The longest error detail shown to the listener.
const DETAIL_LIMIT: usize = 240;

/// An error's text on one line, short enough for a notice. Driver messages
/// can run over several lines.
fn sanitize(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(DETAIL_LIMIT) {
        Some((end, _)) => format!("{}...", &line[..end]),
        None => line,
    }
}

/// What the listener is told when the output will not open: what to do
/// about a Windows error it can name, else the driver's own words.
fn open_failure_text(error: &OpenError) -> String {
    match error {
        OpenError::NoDevice => NO_DEVICE.to_string(),
        other => {
            let detail = other.to_string();
            device_failure_text(&detail).unwrap_or_else(|| sanitize(&detail))
        }
    }
}

/// What the listener is told when a playing output stopped and would not
/// open again: the actual reason, not a generic notice.
fn lost_output_text(error: &OpenError) -> String {
    match error {
        OpenError::NoDevice => NO_DEVICE.to_string(),
        other => {
            let detail = other.to_string();
            device_failure_text(&detail).unwrap_or_else(|| {
                format!("The audio output stopped working: {}", sanitize(&detail))
            })
        }
    }
}

/// What went wrong with the audio device, as far as the listener can act
/// on it. WASAPI reports failures as an HRESULT inside the driver's text,
/// which says nothing to most people ("OS Error -2004287480"); the log keeps
/// that text, the notice says this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    /// Another app holds the device in exclusive mode.
    Busy,
    /// Windows could not create the endpoint, or the device refused every
    /// format: usually a driver that stopped answering.
    Unopenable,
    /// The device was unplugged or disabled.
    Disconnected,
    /// The Windows Audio service is stopped.
    ServiceStopped,
    /// Windows refused access to the device.
    Denied,
    /// Any other Windows error.
    Other,
}

const AUDCLNT_E_DEVICE_INVALIDATED: u32 = 0x8889_0004;
const AUDCLNT_E_UNSUPPORTED_FORMAT: u32 = 0x8889_0008;
const AUDCLNT_E_DEVICE_IN_USE: u32 = 0x8889_000A;
const AUDCLNT_E_ENDPOINT_CREATE_FAILED: u32 = 0x8889_000F;
const AUDCLNT_E_SERVICE_NOT_RUNNING: u32 = 0x8889_0010;
const E_ACCESSDENIED: u32 = 0x8007_0005;

impl Fault {
    const ALL: [Self; 6] = [
        Self::Busy,
        Self::Unopenable,
        Self::Disconnected,
        Self::ServiceStopped,
        Self::Denied,
        Self::Other,
    ];

    /// The fault an HRESULT stands for. `RPC_E_CHANGED_MODE` (COM set up
    /// for another threading model) has nothing the listener can do, so it
    /// is `Other`, with its code to quote.
    fn of(code: u32) -> Self {
        match code {
            AUDCLNT_E_DEVICE_IN_USE => Self::Busy,
            AUDCLNT_E_ENDPOINT_CREATE_FAILED | AUDCLNT_E_UNSUPPORTED_FORMAT => Self::Unopenable,
            AUDCLNT_E_DEVICE_INVALIDATED => Self::Disconnected,
            AUDCLNT_E_SERVICE_NOT_RUNNING => Self::ServiceStopped,
            E_ACCESSDENIED => Self::Denied,
            _ => Self::Other,
        }
    }

    fn text(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Busy => gettext(
                locale,
                "Another app is using your audio device exclusively. Close it, or turn off exclusive mode in Windows sound settings.",
            ),
            Self::Unopenable => gettext(
                locale,
                "Windows couldn't open your audio device. Its driver may not be responding; try another output or restart Windows audio.",
            ),
            Self::Disconnected => gettext(
                locale,
                "The audio device was disconnected. Reconnect it or choose another output.",
            ),
            Self::ServiceStopped => gettext(
                locale,
                "Windows audio isn't running. Start the Windows Audio service, then press play again.",
            ),
            Self::Denied => gettext(
                locale,
                "Windows blocked access to your audio device. Try another output.",
            ),
            Self::Other => gettext(locale, "Can't play on this audio device right now."),
        }
    }
}

/// The HRESULT in an audio error's text: an `os error` code, which std
/// prints as a signed decimal (a Win32 code is widened as
/// `HRESULT_FROM_WIN32`), or a failure code written in hex.
fn hresult(text: &str) -> Option<u32> {
    let lower = text.to_ascii_lowercase();
    let os_error = lower.match_indices("os error ").find_map(|(at, marker)| {
        let rest = &lower[at + marker.len()..];
        let end = rest
            .char_indices()
            .find(|&(index, c)| !(c.is_ascii_digit() || (index == 0 && c == '-')))
            .map_or(rest.len(), |(index, _)| index);
        let code = rest[..end].parse::<i64>().ok()?;
        match code {
            -0x8000_0000..=-1 => Some(code as i32 as u32),
            1..=0xFFFF => Some(0x8007_0000 | code as u32),
            _ => None,
        }
    });
    os_error.or_else(|| {
        lower.match_indices("0x").find_map(|(at, _)| {
            let digits = lower.get(at + 2..at + 10)?;
            let code = u32::from_str_radix(digits, 16).ok()?;
            let whole = !lower[at + 10..].starts_with(|c: char| c.is_ascii_hexdigit());
            (whole && code & 0x8000_0000 != 0).then_some(code)
        })
    })
}

/// A failure code as the notice quotes it.
fn code_text(locale: Locale, code: u32) -> String {
    // Translators: Keep {code} exactly as written. It becomes a Windows error code such as 0x8889000F, shown after an audio device error.
    gettext(locale, "(code {code})").replace("{code}", &format!("0x{code:08X}"))
}

/// The notice for a device error that names a Windows code, in English:
/// the engine reports from its own thread, and [`localized_error`] puts it
/// in the listener's language.
fn device_failure_text(detail: &str) -> Option<String> {
    let code = hresult(detail)?;
    Some(format!(
        "{} {}",
        Fault::of(code).text(Locale::English),
        code_text(Locale::English, code)
    ))
}

/// A notice from [`device_failure_text`] in the listener's language, or
/// `None` for any other message.
pub fn localized_error(locale: Locale, error: &str) -> Option<String> {
    let (sentence, code) = error.rsplit_once(" (code 0x")?;
    let code = code.strip_suffix(')').filter(|hex| hex.len() == 8)?;
    let code = u32::from_str_radix(code, 16).ok()?;
    let fault = Fault::ALL
        .into_iter()
        .find(|fault| fault.text(Locale::English) == sentence)?;
    Some(format!(
        "{} {}",
        fault.text(locale),
        code_text(locale, code)
    ))
}

impl RodioSink {
    pub fn new(
        device: Option<String>,
        on_error: ErrorHook,
        volume: Box<dyn VolumeGetter + Send>,
        buffer_ms: u32,
        control: Arc<AudioControl>,
    ) -> Self {
        Self {
            device,
            output: None,
            on_error,
            volume,
            applied_volume: -1.0,
            buffer_ms,
            control,
            open: open_output,
        }
    }

    fn apply_volume(&mut self) {
        let factor = self.volume.attenuation_factor() as f32;
        if let Some(output) = &self.output
            && factor != self.applied_volume
        {
            output.volume.store(factor.to_bits(), Ordering::Relaxed);
            self.applied_volume = factor;
        }
    }

    /// Opens the output if it is not open, and has it ask for sound. One
    /// attempt.
    fn try_open(&mut self) -> Result<(), OpenError> {
        match &mut self.output {
            Some(output) => {
                if output.run(&self.control)? {
                    self.applied_volume = -1.0;
                }
            }
            None => {
                self.output = Some((self.open)(
                    self.device.as_deref(),
                    self.buffer_ms,
                    &self.control,
                )?);
                self.applied_volume = -1.0;
            }
        }
        Ok(())
    }

    /// As `try_open`, asking again a few times when the device refuses. A
    /// failed attempt lets the output go, so the next one opens afresh
    /// (finding the default output again), and so does the next Play after
    /// a failure that is reported.
    fn open_if_needed(&mut self) -> Result<(), OpenError> {
        let mut failures = 0;
        loop {
            let Err(error) = self.try_open() else {
                return Ok(());
            };
            self.output = None;
            failures += 1;
            let Some(delay) = retry_delay(failures, retryable(&error)) else {
                return Err(error);
            };
            log::warn!(
                "audio output: {error}; trying again in {} ms",
                delay.as_millis()
            );
            thread::sleep(delay);
        }
    }

    /// As `open_if_needed`, reporting a failure to the interface.
    fn ensure_open(&mut self) -> SinkResult<()> {
        self.open_if_needed().map_err(|error| {
            let message = open_failure_text(&error);
            log::error!("cannot open the audio output: {error}");
            (self.on_error)(message.clone());
            SinkError::ConnectionRefused(message)
        })
    }

    /// Opens the output again after the playing one stopped, reporting why
    /// when it will not.
    fn reopen_after_loss(&mut self, cause: &OpenError) -> SinkResult<()> {
        log::warn!("the audio output stopped ({cause}); opening it again");
        self.output = None;
        self.open_if_needed().map_err(|error| {
            let message = lost_output_text(&error);
            log::error!("the audio output stopped and did not open again: {error}");
            (self.on_error)(message.clone());
            SinkError::OnWrite(message)
        })
    }
}

impl Sink for RodioSink {
    /// Never fails: an output that cannot open is reported by the first
    /// `write` instead (#623).
    ///
    /// librespot starts the sink from inside its playing loop and, when
    /// `start` fails, pauses and then carries on as if it were still
    /// playing. It finds itself paused, calls that an invalid state and
    /// exits the process. A failed `write` pauses too, but at a point
    /// where librespot expects it, so playback stops with a message and
    /// the app stays up as a Connect remote.
    fn start(&mut self) -> SinkResult<()> {
        take_precedence();
        // One attempt: the first write asks again, and reports.
        if let Err(error) = self.try_open() {
            log::debug!("audio output not open at start: {error}");
            return Ok(());
        }
        self.apply_volume();
        if let Some(output) = &mut self.output {
            output.transport.fade_in();
            output.sink.play();
        }
        Ok(())
    }

    /// Never fails: librespot exits the process when a sink cannot stop.
    fn stop(&mut self) -> SinkResult<()> {
        if let Some(output) = &mut self.output {
            // The drain below plays the queue out, so the ramp is cut to
            // what is in it. During steady playback that is the whole
            // 50 ms; just after a seek or a track change it is whatever has
            // been decoded since.
            output.transport.fade_out_over(output.queued.frames());
            let deadline = Instant::now() + DRAIN_TIMEOUT;
            while !output.sink.empty() && !output.failed() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            output.sink.pause();
            // With the queue played out, the device can stop asking for
            // sound until Play: a paused app costs no audio work (#636).
            output.device.pause();
            output.transport.close();
            output.fed = false;
            output.last_write = None;
        }
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        let samples = packet
            .samples()
            .map_err(|error| SinkError::OnWrite(error.to_string()))?;
        if self.control.waiting_for_track() {
            // Muting must not remove decoder backpressure. Otherwise cached
            // audio races to EndOfTrack while Connect is still handling the
            // replacement load, and that old event can skip the chosen song.
            // Pace one discarded packet, then let librespot process commands.
            let frames = samples.len() / NUM_CHANNELS as usize;
            thread::sleep(Duration::from_secs_f64(frames as f64 / SAMPLE_RATE as f64));
            return Ok(());
        }
        let samples = converter.f64_to_f32(samples);
        // Sound arriving without a Play first still has a device to go to.
        self.ensure_open()?;
        if let Some(reset) = self.control.take_reset()
            && let Some(output) = &mut self.output
        {
            let sink = Arc::new(rodio::Sink::connect_new(&output.mixer));
            // A seek comes up quickly; a replaced track as slowly as the old
            // one went down. Either fades out over the full length later.
            let envelope = Envelope::rising(output.sample_rate, INTERRUPT_FADE, reset.fade_in());
            self.control.register(&sink, Arc::clone(&envelope));
            output.sink = sink;
            output.envelope = envelope;
            output.queued = Queued::new();
            output.resampler =
                Resampler::new(SAMPLE_RATE, output.sample_rate, NUM_CHANNELS as usize);
            output.fed = false;
            output.last_write = None;
            self.applied_volume = -1.0;
            if reset == Reset::Seek {
                self.control.seek_queued();
            }
        }
        self.apply_volume();
        let Some(output) = &mut self.output else {
            return Err(SinkError::NotConnected(
                "the audio output is not open".into(),
            ));
        };
        let samples = match &mut output.resampler {
            Some(resampler) => resampler.process(&samples),
            None => samples,
        };
        let (channels, samples) = fit_channels(samples, output.channels);
        let now = Instant::now();
        if output.fed && output.sink.empty() && !output.sink.is_paused() {
            let late_ms = output
                .last_write
                .map(|last| now.duration_since(last).as_millis())
                .unwrap_or(0);
            log::warn!("audio queue ran dry; next packet arrived after {late_ms} ms");
        }
        output.transport.fade_in();
        let frames = (samples.len() / usize::from(channels)) as u32;
        let source = rodio::buffer::SamplesBuffer::new(
            channels as rodio::ChannelCount,
            output.sample_rate as rodio::SampleRate,
            samples,
        );
        output
            .queued
            .appended
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        output.sink.append(TransitionSource::new(
            source,
            Arc::clone(&output.envelope),
            Arc::clone(&output.transport),
            Arc::clone(&output.queued),
            frames,
        ));
        output.fed = true;
        output.last_write = Some(now);
        // Let rodio drain a little; without this the whole track would be
        // decoded into memory at once. A full queue sleeps until rodio has
        // played `REFILL` of it, then is topped up in one go.
        let refill = u64::from(output.sample_rate) * REFILL.as_millis() as u64 / 1_000;
        let mut stall = Stall::new(output.queued.consumed(), Instant::now());
        let mut stalled = false;
        let mut lost = None;
        while output.sink.len() > QUEUE_LIMIT {
            if output.failed() {
                // Headphones that connect, a new default output or a device
                // that went away: move to the output to use now and carry on
                // instead of stopping the music. Only an output that will not
                // open stops it.
                match output.run(&self.control) {
                    Ok(replaced) => {
                        if replaced {
                            self.applied_volume = -1.0;
                        }
                        stall = Stall::new(output.queued.consumed(), Instant::now());
                        continue;
                    }
                    Err(error) => {
                        lost = Some(error);
                        break;
                    }
                }
            }
            if stall.observe(output.queued.consumed(), Instant::now()) {
                stalled = true;
                break;
            }
            output.queued.wait_for_room(refill);
        }
        if let Some(cause) = lost {
            // The queued sound went with the stream; the fresh one starts
            // from the next packet.
            self.reopen_after_loss(&cause)?;
        }
        if stalled {
            // A stream can stop asking for sound without reporting an error,
            // and then nothing would ever make room. Start it over.
            log::warn!("the audio output stopped playing for {STALL:?}; opening it again");
            self.output = None;
            self.ensure_open()?;
        }
        self.apply_volume();
        Ok(())
    }
}

/// Puts the decoder thread in the user-initiated QoS class, so macOS keeps
/// it on a performance core and ahead of background work, and queued audio
/// does not run out under load (#88).
#[cfg(target_os = "macos")]
fn take_precedence() {
    const QOS_CLASS_USER_INITIATED: u32 = 0x19;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
    }
    // SAFETY: changes only the calling thread's own QoS class.
    unsafe {
        pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0);
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn take_precedence() {}

/// Raises the Windows decoder thread one step above normal to prevent queued
/// audio from running out under load (#88).
#[cfg(windows)]
fn take_precedence() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    // SAFETY: the current thread's pseudo-handle needs no closing, and the
    // call takes nothing else.
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL);
    }
}

#[derive(Debug, thiserror::Error)]
enum OpenError {
    #[error("{NO_DEVICE}")]
    NoDevice,
    #[error("{0}")]
    Device(fastframe_audio::OpenError),
    /// The stream failed and reopening it left nothing playing.
    #[error("the audio stream stopped and did not start again")]
    Stopped,
}

impl From<fastframe_audio::OpenError> for OpenError {
    fn from(error: fastframe_audio::OpenError) -> Self {
        match error {
            fastframe_audio::OpenError::NoDevice => Self::NoDevice,
            other => Self::Device(other),
        }
    }
}

/// Where Settings asks the output to play.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Named(String),
    Default,
}

/// A blank device name means the system's default.
fn target(preferred: Option<&str>) -> Target {
    match preferred.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => Target::Named(name.to_string()),
        None => Target::Default,
    }
}

/// What the output asks of the device: Spotify's stereo 44.1 kHz first, so
/// nothing is converted, then whatever the device takes. fastframe-audio
/// has no way to insist on stereo: a device that refuses two channels is
/// opened with its own count, which `fit_channels` and `log_stream` handle.
/// The fixed buffer addresses Windows shared-mode underruns (#88);
/// CoreAudio, ALSA, PulseAudio and PipeWire keep their proven driver-selected
/// periods.
fn output_options(device: fastframe_audio::Device, buffer_ms: u32) -> OutputOptions {
    let buffer_ms = buffer_ms.clamp(*BUFFER_MS_RANGE.start(), *BUFFER_MS_RANGE.end());
    OutputOptions {
        device,
        channels: NUM_CHANNELS as u16,
        sample_rate: Some(SAMPLE_RATE),
        buffer: Buffer::FixedOnWindows(BufferSize::Duration(Duration::from_millis(u64::from(
            buffer_ms,
        )))),
        // Only `Device::Default` is followed by fastframe-audio, and the sink
        // opens that only when it cannot tell which endpoint is the default.
        follow_default: true,
        ..OutputOptions::default()
    }
}

fn open_device(
    device: fastframe_audio::Device,
    buffer_ms: u32,
    renderer: MixerRender,
) -> Result<fastframe_audio::Output<MixerRender>, fastframe_audio::OpenError> {
    fastframe_audio::Output::open(output_options(device, buffer_ms), renderer)
}

/// The default output, which fastframe-audio follows itself off Windows.
#[cfg(not(windows))]
fn default_device() -> Result<(fastframe_audio::Device, Option<Follow>), OpenError> {
    Ok((fastframe_audio::Device::Default, None))
}

/// The default output as the endpoint it is now, followed from here on.
#[cfg(windows)]
fn default_device() -> Result<(fastframe_audio::Device, Option<Follow>), OpenError> {
    match endpoint::default_output() {
        Ok(Some(endpoint)) => Ok((
            fastframe_audio::Device::Named(endpoint.name.clone()),
            Some(Follow::start(endpoint)),
        )),
        Ok(None) => Err(OpenError::NoDevice),
        Err(error) => {
            log::warn!("cannot tell which audio output is the default ({error}); asking for it");
            Ok((fastframe_audio::Device::Default, None))
        }
    }
}

/// Whether fastframe-audio lists a device by this name. A device that
/// cannot be listed is left to fastframe-audio to look for.
fn listed(name: &str) -> bool {
    fastframe_audio::output_device_names()
        .map(|names| names.iter().any(|listed| listed == name))
        .unwrap_or(true)
}

fn open_output(
    preferred: Option<&str>,
    buffer_ms: u32,
    control: &AudioControl,
) -> Result<Output, OpenError> {
    let (device, follow) = match target(preferred) {
        Target::Named(name) if listed(&name) => (fastframe_audio::Device::Named(name), None),
        Target::Named(name) => {
            log::warn!("audio device {name:?} is not available; using the default output");
            default_device()?
        }
        Target::Default => default_device()?,
    };
    let made = MixerSlot::default();
    let mix = Arc::new(Mutex::new(Mix::default()));
    let volume = Arc::new(AtomicU32::new(0.0f32.to_bits()));
    let device = open_device(device, buffer_ms, MixerRender::new(&volume, &mix, &made))?;
    log_stream(
        "opened",
        device.device_name(),
        device.sample_rate(),
        device.channels(),
    );
    // The open configured the renderer, which made the mixer.
    let (mixer, sample_rate, channels) = made
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .ok_or(OpenError::NoDevice)?;
    let mut output = Output {
        device,
        volume,
        mix,
        made,
        sink: Arc::new(rodio::Sink::connect_new(&mixer)),
        mixer: mixer.clone(),
        sample_rate,
        channels,
        resampler: None,
        envelope: Envelope::open(sample_rate, INTERRUPT_FADE),
        transport: Envelope::closed(sample_rate, TRANSPORT_FADE),
        queued: Queued::new(),
        fed: false,
        last_write: None,
        follow,
        buffer_ms,
    };
    output.attach((mixer, sample_rate, channels), control);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn volume_changes_take_thirty_ms_at_each_output_rate() {
        for rate in [44_100, 48_000, 96_000] {
            let frames = rate * 30 / 1_000;
            let mut ramp = VolumeRamp::default();
            for target in [1.0, 0.25, 0.0, 0.8] {
                let start = ramp.current;
                for frame in 0..frames {
                    let gain = ramp.next_gain(target, rate);
                    let expected = start + (target - start) * frame as f32 / frames as f32;
                    assert!((gain - expected).abs() < 0.0001);
                }
                assert_eq!(ramp.next_gain(target, rate), target);
                assert_eq!(ramp.remaining, 0);
            }
        }
    }

    #[test]
    fn retargeting_volume_continues_from_the_current_gain() {
        let mut ramp = VolumeRamp::default();
        for _ in 0..480 {
            ramp.next_gain(1.0, 48_000);
        }
        let current = ramp.current;
        assert!(current > 0.0 && current < 1.0);
        assert_eq!(ramp.next_gain(0.0, 48_000), current);
        for _ in 1..1440 {
            ramp.next_gain(0.0, 48_000);
        }
        assert_eq!(ramp.next_gain(0.0, 48_000), 0.0);
    }

    #[test]
    fn rendered_volume_is_stereo_linked_and_spans_callbacks() {
        let volume = Arc::new(AtomicU32::new(1.0f32.to_bits()));
        let made = MixerSlot::default();
        let mut render = MixerRender::new(&volume, &Arc::default(), &made);
        render.configure(48_000, 2);
        let (mixer, _, _) = made.lock().unwrap().take().unwrap();
        mixer.add(rodio::buffer::SamplesBuffer::new(
            2,
            48_000,
            vec![1.0; 10_000],
        ));
        let mut rising = Vec::new();
        for _ in 0..6 {
            let mut block = [0.0; 480];
            render.render(&mut block);
            rising.extend(block);
        }
        assert_eq!(rising[0], 0.0);
        assert!(rising.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(
            rising
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| pair[0] == pair[1])
        );
        let mut settled = [0.0; 2];
        render.render(&mut settled);
        assert_eq!(settled, [1.0; 2]);
        volume.store(0.0f32.to_bits(), Ordering::Relaxed);
        let mut falling = vec![0.0; 2882];
        render.render(&mut falling);
        assert_eq!(falling[0], 1.0);
        assert_eq!(&falling[2880..], &[0.0; 2]);
        assert!(falling.windows(2).all(|pair| pair[0] >= pair[1]));
        assert!(
            falling
                .as_chunks::<2>()
                .0
                .iter()
                .all(|pair| pair[0] == pair[1])
        );
    }
    /// The buffer setting reaches the device on Windows only (#88), and a
    /// settings file with a wild number in it still opens a stream: the
    /// range is the range whoever wrote the file thought of.
    #[test]
    fn the_buffer_follows_the_setting_within_its_range() {
        let buffer = |ms| output_options(fastframe_audio::Device::Default, ms).buffer;
        let fixed = |ms| Buffer::FixedOnWindows(BufferSize::Duration(Duration::from_millis(ms)));
        assert_eq!(buffer(100), fixed(100));
        assert_eq!(buffer(0), fixed(u64::from(*BUFFER_MS_RANGE.start())));
        assert_eq!(buffer(100_000), fixed(u64::from(*BUFFER_MS_RANGE.end())));
    }

    /// Spotify's own format is asked for first, so nothing is converted, and
    /// a blank device name means the system's default.
    #[test]
    fn the_output_asks_for_spotifys_format_on_the_chosen_device() {
        let named = fastframe_audio::Device::Named("USB DAC".into());
        let options = output_options(named.clone(), DEFAULT_BUFFER_MS);
        assert_eq!(options.device, named);
        assert_eq!(options.sample_rate, Some(SAMPLE_RATE));
        assert_eq!(options.channels, NUM_CHANNELS as u16);
        assert_eq!(target(Some(" USB DAC ")), Target::Named("USB DAC".into()));
        for blank in [None, Some(""), Some("  ")] {
            assert_eq!(target(blank), Target::Default);
        }
    }

    /// A stream reopened in the format the mixer already plays keeps it, so
    /// what is queued carries on; another rate gets a new mixer for the sink
    /// to pick up.
    #[test]
    fn only_a_new_format_makes_a_new_mixer() {
        let made = MixerSlot::default();
        let mut render = MixerRender::new(
            &Arc::new(AtomicU32::new(1.0f32.to_bits())),
            &Arc::default(),
            &made,
        );
        let mut out = [1.0; 8];
        render.render(&mut out);
        assert_eq!(out, [0.0; 8], "silence before there is a mixer");

        render.configure(44_100, 2);
        let (mixer, rate, _) = made.lock().unwrap().take().expect("a first mixer");
        assert_eq!(rate, 44_100);
        render.configure(44_100, 2);
        assert!(made.lock().unwrap().is_none(), "the same format keeps it");
        render.configure(48_000, 2);
        assert_eq!(
            made.lock().unwrap().as_ref().map(|made| made.1),
            Some(48_000)
        );
        drop(mixer);
    }

    /// A machine without audio (CI, a PC with nothing plugged in) must get
    /// an error and a message for the interface, never a panic. A machine
    /// with audio opens its default device.
    #[test]
    fn starting_without_a_device_is_an_error_not_a_panic() {
        let reported: Arc<Mutex<Option<String>>> = Arc::default();
        let store = Arc::clone(&reported);
        let mut sink = RodioSink::new(
            Some("no such device".into()),
            Arc::new(move |message| *store.lock().unwrap() = Some(message)),
            Box::new(librespot_playback::mixer::NoOpVolume),
            DEFAULT_BUFFER_MS,
            AudioControl::new(DEFAULT_BUFFER_MS),
        );
        match sink.start() {
            Ok(()) => assert!(reported.lock().unwrap().is_none()),
            Err(SinkError::ConnectionRefused(message)) => {
                assert_eq!(reported.lock().unwrap().as_deref(), Some(message.as_str()));
            }
            Err(other) => panic!("unexpected error: {other}"),
        }
        assert!(sink.stop().is_ok());
    }

    /// #636: a paused player stops the device asking for sound, and Play,
    /// or sound arriving without one, starts it again. Needs an output, so
    /// a machine without audio has nothing to check.
    #[test]
    fn pause_stops_the_device_and_play_starts_it_again() {
        let mut sink = RodioSink::new(
            None,
            Arc::new(|_| {}),
            Box::new(librespot_playback::mixer::NoOpVolume),
            DEFAULT_BUFFER_MS,
            AudioControl::new(DEFAULT_BUFFER_MS),
        );
        assert!(sink.start().is_ok());
        let running = |sink: &RodioSink| {
            sink.output
                .as_ref()
                .map(|output| !output.device.is_paused())
        };
        if running(&sink).is_none() {
            return;
        }
        let mut converter = Converter::new(None);
        // Silence, so a test run plays nothing on the speakers.
        let packet = || AudioPacket::Samples(vec![0.0; 441 * NUM_CHANNELS as usize]);
        sink.write(packet(), &mut converter).unwrap();
        assert_eq!(running(&sink), Some(true));

        assert!(sink.stop().is_ok());
        assert_eq!(running(&sink), Some(false), "paused, the device is quiet");
        assert!(sink.stop().is_ok(), "stopping twice is harmless");

        assert!(sink.start().is_ok());
        assert_eq!(running(&sink), Some(true), "Play starts it again");

        assert!(sink.stop().is_ok());
        sink.write(packet(), &mut converter).unwrap();
        assert_eq!(running(&sink), Some(true), "and so does sound");
        assert!(sink.stop().is_ok());
    }

    fn no_device(_: Option<&str>, _: u32, _: &AudioControl) -> Result<Output, OpenError> {
        Err(OpenError::NoDevice)
    }

    /// #623: a PC with no output at all. librespot exits the process when
    /// `start` fails from its playing loop, but pauses cleanly when `write`
    /// fails, so the failure has to surface from `write`, reported to the
    /// interface once per attempt to play.
    #[test]
    fn with_no_output_at_all_playing_fails_at_the_first_packet_not_at_start() {
        let reported: Arc<Mutex<Vec<String>>> = Arc::default();
        let store = Arc::clone(&reported);
        let mut sink = RodioSink {
            open: no_device,
            ..RodioSink::new(
                None,
                Arc::new(move |message| store.lock().unwrap().push(message)),
                Box::new(librespot_playback::mixer::NoOpVolume),
                DEFAULT_BUFFER_MS,
                AudioControl::new(DEFAULT_BUFFER_MS),
            )
        };
        let mut converter = Converter::new(None);
        let packet = || AudioPacket::Samples(vec![0.0; 441 * NUM_CHANNELS as usize]);

        for attempt in 1..=2 {
            assert!(sink.start().is_ok(), "librespot exits when start fails");
            assert_eq!(reported.lock().unwrap().len(), attempt - 1);

            let Err(SinkError::ConnectionRefused(message)) = sink.write(packet(), &mut converter)
            else {
                panic!("the first packet must report the missing output");
            };
            assert_eq!(message, NO_DEVICE);
            assert_eq!(reported.lock().unwrap().len(), attempt);
            assert_eq!(reported.lock().unwrap().last().unwrap(), NO_DEVICE);

            // librespot pauses on the failed write, which stops the sink.
            assert!(sink.stop().is_ok());
        }
    }

    /// A rate that keeps a ramp short enough to step through in a test.
    const RATE: u32 = 1_000;

    /// Ramps that stay out of the way, for tests about something else.
    fn wide_open() -> (Arc<Envelope>, Arc<Envelope>) {
        (
            Envelope::open(RATE, INTERRUPT_FADE),
            Envelope::open(RATE, TRANSPORT_FADE),
        )
    }

    /// A chunk of full scale sound, counted into `queued` the way `write`
    /// counts one, and shaped by the ramps it is handed.
    fn chunk(
        frames: u32,
        interrupt: &Arc<Envelope>,
        transport: &Arc<Envelope>,
        queued: &Arc<Queued>,
    ) -> TransitionSource {
        queued
            .appended
            .fetch_add(u64::from(frames), Ordering::Relaxed);
        TransitionSource::new(
            rodio::buffer::SamplesBuffer::new(
                NUM_CHANNELS.into(),
                RATE,
                vec![1.0; frames as usize * NUM_CHANNELS as usize],
            ),
            Arc::clone(interrupt),
            Arc::clone(transport),
            Arc::clone(queued),
            frames,
        )
    }

    /// The gain each frame comes out at. The sound is full scale, so every
    /// sample is the gain that shaped it.
    fn gains(frames: u32, interrupt: &Arc<Envelope>, transport: &Arc<Envelope>) -> Vec<f32> {
        chunk(frames, interrupt, transport, &Queued::new())
            .step_by(NUM_CHANNELS as usize)
            .collect()
    }

    /// Asserts a ramp still sounds for every one of `frames`, and is silent
    /// on the frame after.
    fn falls_silent_after(envelope: &Envelope, frames: u32) {
        for step in 0..frames {
            assert!(envelope.next_gain() > 0.0, "silent {step} frames early");
        }
        assert_eq!(envelope.next_gain(), 0.0);
        assert!(envelope.silent());
    }

    #[test]
    fn an_interrupted_decoder_cannot_race_to_the_end_while_a_new_track_loads() {
        let control = AudioControl::new(DEFAULT_BUFFER_MS);
        control.interrupt();
        let mut sink = RodioSink::new(
            None,
            Arc::new(|error| panic!("no audio device should be opened: {error}")),
            Box::new(librespot_playback::mixer::NoOpVolume),
            DEFAULT_BUFFER_MS,
            control,
        );
        let mut converter = Converter::new(None);
        let frames = SAMPLE_RATE as usize / 100;
        let started = Instant::now();
        for _ in 0..4 {
            sink.write(
                AudioPacket::Samples(vec![0.0; frames * NUM_CHANNELS as usize]),
                &mut converter,
            )
            .unwrap();
        }
        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "discarded audio must retain backpressure until the new load reaches the decoder"
        );
        assert!(sink.output.is_none());
    }

    #[test]
    fn confirmed_seek_discards_the_old_position_without_gating_new_packets() {
        let control = AudioControl::new(DEFAULT_BUFFER_MS);
        let (sink, mut output) = rodio::Sink::new();
        let sink = Arc::new(sink);
        let (interrupt, transport) = wide_open();
        let queued = Queued::new();
        control.register(&sink, Arc::clone(&interrupt));
        sink.append(chunk(500, &interrupt, &transport, &queued));
        assert_eq!(output.next(), Some(1.0));

        control.handle_player_event(&PlayerEvent::Seeked {
            play_request_id: 1,
            track_id: librespot_core::SpotifyUri::from_uri("spotify:track:14XWXWv5FoCbFzLksawpEe")
                .unwrap(),
            position_ms: 90_000,
        });

        // Rodio checks stop every 5 ms of output. After that, none of the
        // half-second of sound from before the seek may still play.
        output
            .by_ref()
            .take(20 * NUM_CHANNELS as usize)
            .for_each(drop);
        assert!(output.take(50).all(|sample| sample == 0.0));
        assert_eq!(queued.frames(), 0);
        assert!(!control.waiting_for_track());
        assert_eq!(
            control.take_reset(),
            Some(Reset::Seek),
            "the next packet gets a fresh queue"
        );
    }

    #[test]
    fn track_changes_and_position_updates_preserve_gapless_queued_audio() {
        use librespot_metadata::audio::item::{AudioItem, UniqueFields};

        let control = AudioControl::new(DEFAULT_BUFFER_MS);
        let (sink, mut output) = rodio::Sink::new();
        let sink = Arc::new(sink);
        let (interrupt, transport) = wide_open();
        control.register(&sink, Arc::clone(&interrupt));
        sink.append(chunk(500, &interrupt, &transport, &Queued::new()));
        assert_eq!(output.next(), Some(1.0));
        let track_id =
            librespot_core::SpotifyUri::from_uri("spotify:track:14XWXWv5FoCbFzLksawpEe").unwrap();
        let item = AudioItem {
            track_id: track_id.clone(),
            uri: track_id.to_uri().unwrap(),
            files: Default::default(),
            name: "Next song".into(),
            covers: vec![],
            language: vec![],
            duration_ms: 200_000,
            is_explicit: false,
            availability: Ok(()),
            alternatives: None,
            unique_fields: UniqueFields::Track {
                artists: Default::default(),
                album: "Album".into(),
                album_artists: vec![],
                popularity: 0,
                number: 1,
                disc_number: 1,
            },
        };
        for event in [
            PlayerEvent::TrackChanged {
                audio_item: Box::new(item),
            },
            PlayerEvent::PositionCorrection {
                play_request_id: 1,
                track_id: track_id.clone(),
                position_ms: 100,
            },
            PlayerEvent::PositionChanged {
                play_request_id: 1,
                track_id,
                position_ms: 200,
            },
        ] {
            control.handle_player_event(&event);
            assert!(output.by_ref().take(100).all(|sample| sample == 1.0));
            assert_eq!(control.take_reset(), None);
        }
    }

    #[test]
    fn a_previous_that_rewinds_releases_the_interrupted_track_gate() {
        let control = AudioControl::new(DEFAULT_BUFFER_MS);
        control.interrupt();
        assert!(control.waiting_for_track());

        control.handle_player_event(&PlayerEvent::Seeked {
            play_request_id: 1,
            track_id: librespot_core::SpotifyUri::from_uri("spotify:track:14XWXWv5FoCbFzLksawpEe")
                .unwrap(),
            position_ms: 0,
        });
        assert!(!control.waiting_for_track());
        // The rewind comes up as gently as the interrupted track went down.
        assert_eq!(control.take_reset(), Some(Reset::Interrupt));
    }

    #[test]
    fn an_interrupted_signal_fades_out_and_a_replacement_fades_in() {
        let frames = fade_frames(RATE, INTERRUPT_FADE);
        let (interrupt, transport) = wide_open();
        interrupt.fade_out();

        let faded = gains(frames + 2, &interrupt, &transport);
        assert_eq!(faded[0], 1.0);
        assert_eq!(faded[frames as usize], 0.0);

        let incoming = Envelope::rising(RATE, INTERRUPT_FADE, INTERRUPT_FADE);
        let risen = gains(frames + 2, &incoming, &transport);
        assert_eq!(risen[0], 0.0);
        assert_eq!(risen[frames as usize], 1.0);
    }

    /// One gain per frame rather than per sample, so the two channels of a
    /// frame stay level with each other.
    #[test]
    fn both_channels_of_a_frame_share_a_gain() {
        let (interrupt, transport) = wide_open();
        interrupt.fade_out();

        let played: Vec<_> = chunk(8, &interrupt, &transport, &Queued::new()).collect();
        for pair in played.chunks(NUM_CHANNELS as usize) {
            assert_eq!(pair[0], pair[1]);
        }
    }

    /// A fresh output has played nothing, so the first Play must have silence
    /// to come up from rather than starting already open.
    #[test]
    fn the_first_play_ramps_up_instead_of_starting_open() {
        let transport = Envelope::closed(RATE, TRANSPORT_FADE);
        assert!(transport.silent());
        for _ in 0..fade_frames(RATE, TRANSPORT_FADE) {
            assert_eq!(transport.next_gain(), 0.0);
        }

        transport.fade_in();
        assert_eq!(transport.next_gain(), 0.0);
        assert!(transport.next_gain() > 0.0);
    }

    #[test]
    fn a_pause_reaches_silence_only_after_the_whole_ramp() {
        let transport = Envelope::open(RATE, TRANSPORT_FADE);
        transport.fade_out();
        falls_silent_after(&transport, fade_frames(RATE, TRANSPORT_FADE));
    }

    /// An underrun leaves the pause with no frames to fade through,
    /// so the ramp never moves and the level is still up.
    /// Settling it at the stop is what keeps the next Play coming up
    /// from silence rather than resuming at full gain.
    #[test]
    fn a_pause_with_nothing_queued_still_resumes_from_silence() {
        let transport = Envelope::open(RATE, TRANSPORT_FADE);
        transport.fade_out_over(0);
        // No frame is pulled here, because there is none to pull. That is
        // the underrun, and it leaves the ramp exactly where it started.
        assert!(!transport.silent());

        transport.close();
        assert!(transport.silent());

        transport.fade_in();
        assert_eq!(transport.next_gain(), 0.0);
        assert!(transport.next_gain() > 0.0);
    }

    /// The ramp is clocked by the sound, not by the wall, so it cannot run
    /// past the audio it is shaping however long it is left waiting.
    #[test]
    fn the_fade_advances_with_the_music_not_the_clock() {
        let transport = Envelope::open(RATE, TRANSPORT_FADE);
        transport.fade_out();
        thread::sleep(TRANSPORT_FADE * 2);
        assert_eq!(transport.next_gain(), 1.0);
    }

    /// Skipping during a pause rides both ramps at once, and the shorter one
    /// decides when silence arrives.
    #[test]
    fn a_skip_during_a_pause_carries_both_ramps() {
        let frames = fade_frames(RATE, INTERRUPT_FADE);
        let (interrupt, transport) = wide_open();
        interrupt.fade_out();
        transport.fade_out();

        let faded = gains(frames + 2, &interrupt, &transport);
        assert_eq!(faded[0], 1.0);
        assert_eq!(faded[frames as usize], 0.0);
        assert!(faded.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    #[test]
    fn a_short_queue_still_gets_a_whole_ramp() {
        let left = 20;
        assert!(left < fade_frames(RATE, TRANSPORT_FADE));

        let transport = Envelope::open(RATE, TRANSPORT_FADE);
        transport.fade_out_over(u64::from(left));
        falls_silent_after(&transport, left);
    }

    #[test]
    fn a_deep_queue_does_not_stretch_the_ramp() {
        let nominal = fade_frames(RATE, TRANSPORT_FADE);
        let transport = Envelope::open(RATE, TRANSPORT_FADE);
        transport.fade_out_over(u64::from(nominal) * 10);
        falls_silent_after(&transport, nominal);
    }

    #[test]
    fn playing_a_chunk_takes_it_out_of_the_count() {
        let queued = Queued::new();
        let (interrupt, transport) = wide_open();

        let played = chunk(40, &interrupt, &transport, &queued);
        assert_eq!(queued.frames(), 40);
        assert_eq!(played.count(), 40 * NUM_CHANNELS as usize);
        assert_eq!(queued.frames(), 0);
    }

    /// rodio discards whole sources on a track change. Their frames never
    /// play, so without settling up on drop the count would keep claiming
    /// sound that no longer exists.
    #[test]
    fn a_discarded_chunk_stops_counting_as_queued() {
        let queued = Queued::new();
        let (interrupt, transport) = wide_open();

        drop(chunk(40, &interrupt, &transport, &queued));
        assert_eq!(queued.frames(), 0);
    }

    /// A writer waiting for room sleeps through the chunks that leave the
    /// queue above its level, and the one that reaches it wakes the writer,
    /// once. Polling instead woke the decoder thread every 10 ms.
    #[test]
    fn a_waiting_writer_wakes_once_the_queue_has_drained_enough() {
        let queued = Queued::new();
        let (interrupt, transport) = wide_open();
        let mut chunks: Vec<_> = (0..4)
            .map(|_| chunk(10, &interrupt, &transport, &queued))
            .collect();
        queued.wake_at.store(20, Ordering::SeqCst);

        drop(chunks.remove(0));
        assert_eq!(queued.frames(), 30);
        assert_eq!(queued.wake_at.load(Ordering::SeqCst), 20, "still asleep");

        drop(chunks.remove(0));
        assert_eq!(queued.wake_at.load(Ordering::SeqCst), 0, "woken, once");
        // The chunks were queued from this thread, so the wake is its own:
        // the token is waiting, and parking returns at once.
        let started = Instant::now();
        thread::park_timeout(Duration::from_secs(5));
        assert!(started.elapsed() < Duration::from_secs(1));

        drop(chunks);
        assert_eq!(queued.frames(), 0);
    }

    /// An output whose queue keeps moving is never stuck however long the
    /// wait; one that finishes nothing for `STALL` is, and the clock starts
    /// over each time it moves.
    #[test]
    fn only_an_output_that_stops_playing_counts_as_stuck() {
        let start = Instant::now();
        let mut stall = Stall::new(0, start);
        assert!(!stall.observe(0, start + STALL / 2));
        assert!(!stall.observe(441, start + STALL), "it moved");
        assert!(!stall.observe(441, start + STALL + STALL / 2));
        assert!(stall.observe(441, start + STALL * 2), "nothing for STALL");
        assert!(!stall.observe(882, start + STALL * 3), "moving again");
    }

    /// The time a sleeping system stood still is not the output's fault:
    /// waking up starts the count again, and only an output that then
    /// finishes nothing for `STALL` is stuck.
    #[test]
    fn a_system_that_slept_is_not_a_stuck_output() {
        let start = Instant::now();
        let woke = start + Duration::from_secs(7);
        let mut stall = Stall::new(0, start);
        assert!(!stall.observe(0, start + QUEUE_WAIT));
        assert!(!stall.observe(0, woke), "asleep, not stuck");
        assert!(!stall.observe(0, woke + STALL / 2));
        assert!(stall.observe(0, woke + STALL), "stuck after waking");
    }

    /// A stream that has stopped consuming finishes no chunk, so the wait
    /// gives up by itself and `write` gets to look at the device.
    #[test]
    fn waiting_for_room_gives_up_when_nothing_plays() {
        let queued = Queued::new();
        queued.appended.store(1_000, Ordering::SeqCst);
        let started = Instant::now();
        queued.wait_for_room(100);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(queued.wake_at.load(Ordering::SeqCst), 0);
    }

    fn always_stopped(_: Option<&str>, _: u32, _: &AudioControl) -> Result<Output, OpenError> {
        STOPPED_OPENS.fetch_add(1, Ordering::SeqCst);
        Err(OpenError::Stopped)
    }

    static STOPPED_OPENS: AtomicU32 = AtomicU32::new(0);

    /// A device that is changing hands is asked a few more times, briefly,
    /// before playing fails; a failure that cannot pass is reported at once.
    #[test]
    fn a_refused_open_is_retried_a_few_times_with_a_short_wait() {
        let waits: Vec<_> = (1..=OPEN_ATTEMPTS + 1)
            .map(|failures| retry_delay(failures, true))
            .collect();
        let last = OPEN_ATTEMPTS as usize - 1;
        assert!(waits[..last].iter().all(|wait| *wait == Some(OPEN_RETRY)));
        assert_eq!(waits[last], None, "then it is reported");
        assert_eq!(waits[last + 1], None);
        let total: Duration = waits.iter().flatten().sum();
        assert!(
            total <= Duration::from_secs(1),
            "{total:?} holds up the player"
        );
        assert_eq!(retry_delay(1, false), None);

        assert!(retryable(&OpenError::NoDevice));
        assert!(retryable(&OpenError::Stopped));
        assert!(retryable(&OpenError::Device(
            fastframe_audio::OpenError::NoDevice
        )));
    }

    /// #689, #810: an output that keeps refusing is reported with what
    /// actually went wrong, after the retries, and is let go, so the next
    /// Play opens afresh instead of meeting the same broken stream.
    #[test]
    fn an_output_that_keeps_refusing_reports_its_own_error_and_play_tries_again() {
        let reported: Arc<Mutex<Vec<String>>> = Arc::default();
        let store = Arc::clone(&reported);
        let mut sink = RodioSink {
            open: always_stopped,
            ..RodioSink::new(
                None,
                Arc::new(move |message| store.lock().unwrap().push(message)),
                Box::new(librespot_playback::mixer::NoOpVolume),
                DEFAULT_BUFFER_MS,
                AudioControl::new(DEFAULT_BUFFER_MS),
            )
        };
        let mut converter = Converter::new(None);
        let packet = || AudioPacket::Samples(vec![0.0; 441 * NUM_CHANNELS as usize]);
        for attempt in 1..=2 {
            assert!(sink.write(packet(), &mut converter).is_err());
            assert_eq!(
                STOPPED_OPENS.load(Ordering::SeqCst),
                attempt * OPEN_ATTEMPTS
            );
            assert!(sink.output.is_none());
            let reported = reported.lock().unwrap();
            assert_eq!(reported.len() as u32, attempt);
            assert_eq!(
                reported.last().unwrap(),
                &OpenError::Stopped.to_string(),
                "the real reason, not a generic notice"
            );
        }
    }

    #[test]
    fn a_lost_output_names_the_reason_it_did_not_come_back() {
        assert_eq!(lost_output_text(&OpenError::NoDevice), NO_DEVICE);
        assert_eq!(
            lost_output_text(&OpenError::Stopped),
            "The audio output stopped working: the audio stream stopped and did not start again"
        );
        assert_eq!(open_failure_text(&OpenError::NoDevice), NO_DEVICE);
    }

    /// Driver messages can run over lines and on and on; a notice gets one
    /// line of reasonable length.
    #[test]
    fn error_details_are_one_short_line() {
        assert_eq!(
            sanitize("Failed to get audio client:\r\n  Operation\tfailed "),
            "Failed to get audio client: Operation failed"
        );
        let short = sanitize(&"\u{e9}".repeat(DETAIL_LIMIT * 2));
        assert_eq!(short.chars().count(), DETAIL_LIMIT + 3);
        assert!(short.ends_with("..."));
    }

    /// The HRESULT is found however the driver's text spells it: std's
    /// signed `os error`, a Win32 code, or hex.
    #[test]
    fn an_audio_error_names_its_windows_code() {
        let reported = "Failed to initialize audio client: OS Error -2004287480 \
             (FormatMessageW() returned error 317) (os error -2004287480)";
        assert_eq!(hresult(reported), Some(AUDCLNT_E_UNSUPPORTED_FORMAT));
        assert_eq!(
            hresult("Failed to get audio client (os error -2004287473)"),
            Some(AUDCLNT_E_ENDPOINT_CREATE_FAILED)
        );
        assert_eq!(
            hresult("Access is denied. (os error 5)"),
            Some(E_ACCESSDENIED)
        );
        assert_eq!(
            hresult("CoInitializeEx: HRESULT(0x80010106)"),
            Some(0x8001_0106)
        );
        assert_eq!(
            hresult("device busy 0x8889000a"),
            Some(AUDCLNT_E_DEVICE_IN_USE)
        );
        assert_eq!(
            hresult("the audio stream stopped and did not start again"),
            None
        );
        assert_eq!(hresult("buffer at 0x0000ff00"), None, "not a failure code");
        assert_eq!(hresult("address 0x8889000a12"), None, "not a 32-bit code");
    }

    #[test]
    fn common_windows_audio_codes_are_told_in_plain_words() {
        let cases = [
            (0x8889_0008, Fault::Unopenable),
            (0x8889_000F, Fault::Unopenable),
            (0x8889_000A, Fault::Busy),
            (0x8889_0004, Fault::Disconnected),
            (0x8889_0010, Fault::ServiceStopped),
            (0x8007_0005, Fault::Denied),
            (0x8001_0106, Fault::Other),
            (0x8000_4005, Fault::Other),
        ];
        for (code, fault) in cases {
            assert_eq!(Fault::of(code), fault, "0x{code:08X}");
        }
    }

    /// #689 asked for the real reason; for a Windows code that is what to do
    /// about it and the exact code, and the raw text stays in the log.
    #[test]
    fn a_device_that_will_not_open_is_explained_with_its_code() {
        let detail = "Cannot open the audio output: Failed to initialize audio client: OS Error -2004287473 (FormatMessageW() returned error 317) (os error -2004287473)";
        let message = device_failure_text(detail).unwrap();
        assert_eq!(
            message,
            "Windows couldn't open your audio device. Its driver may not be responding; try another output or restart Windows audio. (code 0x8889000F)"
        );
        assert_eq!(
            device_failure_text("Something odd (os error -2147467259)").as_deref(),
            Some("Can't play on this audio device right now. (code 0x80004005)")
        );
        assert_eq!(
            device_failure_text("The requested device is no longer available."),
            None,
            "without a code the driver's own words stay"
        );
    }

    #[test]
    fn an_explained_audio_error_is_shown_in_the_users_language() {
        for fault in Fault::ALL {
            let english = format!("{} (code 0x88890004)", fault.text(Locale::English));
            assert_eq!(
                localized_error(Locale::English, &english).as_deref(),
                Some(english.as_str())
            );
            let german = localized_error(Locale::German, &english).unwrap();
            assert_ne!(german, english, "{fault:?}");
            assert!(german.ends_with(" 0x88890004)"), "{german}");
        }
        assert_eq!(localized_error(Locale::German, NO_DEVICE), None);
        assert_eq!(
            localized_error(Locale::German, "Something else (code 0x88890004)"),
            None
        );
    }

    fn endpoint(id: &str, name: &str) -> Endpoint {
        Endpoint {
            id: id.into(),
            name: name.into(),
        }
    }

    /// The stream follows the default to another endpoint, or to the same
    /// one under a new name, and otherwise stays put: with no default at
    /// all, or on a default that already refused to open.
    #[test]
    fn a_stream_moves_only_when_the_default_is_another_endpoint() {
        let speakers = endpoint("{0.0.0}.{speakers}", "Speakers (Realtek)");
        let headphones = endpoint("{0.0.0}.{bt}", "Headphones (WH-1000XM4)");
        assert!(!default_moved(&speakers, Some(&speakers), None));
        assert!(!default_moved(&speakers, None, None));
        assert!(default_moved(&speakers, Some(&headphones), None));
        assert!(!default_moved(
            &speakers,
            Some(&headphones),
            Some(&headphones.id)
        ));
        assert!(default_moved(
            &speakers,
            Some(&endpoint(&speakers.id, "Speakers (renamed)")),
            None
        ));
    }

    /// A refused default is not tried at every write, but it is tried again
    /// once the default has come back and moved to it a second time.
    #[test]
    fn a_declined_default_is_tried_again_after_the_default_returns() {
        let speakers = endpoint("speakers", "Speakers");
        let headphones = endpoint("bt", "Headphones");
        let mut follow = Follow {
            opened: speakers.clone(),
            latest: Arc::new(Mutex::new(Some(headphones.clone()))),
            declined: Some(headphones.id.clone()),
        };
        assert_eq!(follow.moved(), None);
        follow.noted(&speakers);
        assert_eq!(follow.moved(), None);
        assert_eq!(follow.declined, None);
        follow.noted(&headphones);
        assert_eq!(follow.moved(), Some(headphones));
    }

    /// #343: a seek comes up in a few milliseconds, a replaced track over
    /// the full interruption fade, and when both are pending the gentler
    /// one wins.
    #[test]
    fn a_seek_fades_in_quickly_and_an_interruption_slowly() {
        assert_eq!(Reset::Seek.fade_in(), SEEK_FADE);
        assert_eq!(Reset::Interrupt.fade_in(), INTERRUPT_FADE);
        assert!(SEEK_FADE < INTERRUPT_FADE);

        let seeked = PlayerEvent::Seeked {
            play_request_id: 1,
            track_id: librespot_core::SpotifyUri::from_uri("spotify:track:14XWXWv5FoCbFzLksawpEe")
                .unwrap(),
            position_ms: 30_000,
        };
        let control = AudioControl::new(DEFAULT_BUFFER_MS);
        control.seek_requested();
        control.handle_player_event(&seeked);
        assert_eq!(control.take_reset(), Some(Reset::Seek));
        assert_eq!(control.take_reset(), None, "taken once");

        control.interrupt();
        assert_eq!(control.take_reset(), Some(Reset::Interrupt));

        control.track_changed();
        control.interrupt();
        control.handle_player_event(&seeked);
        assert_eq!(control.take_reset(), Some(Reset::Interrupt));
    }

    /// The quick rise after a seek leaves the envelope's own length alone,
    /// so skipping away later still fades out over the full length.
    #[test]
    fn a_quick_rise_still_fades_out_over_the_full_length() {
        let rise = fade_frames(RATE, SEEK_FADE);
        let envelope = Envelope::rising(RATE, INTERRUPT_FADE, SEEK_FADE);
        assert_eq!(envelope.next_gain(), 0.0);
        for _ in 1..rise {
            envelope.next_gain();
        }
        assert_eq!(envelope.next_gain(), 1.0);
        envelope.fade_out();
        falls_silent_after(&envelope, fade_frames(RATE, INTERRUPT_FADE));
    }

    /// #804: rodio makes stereo mono by dropping the right channel, so a
    /// mono output gets both channels mixed; any other output takes stereo
    /// as it is.
    #[test]
    fn a_mono_output_hears_both_channels() {
        let stereo = vec![1.0, 0.0, 0.25, 0.75, -0.5, 0.5];
        assert_eq!(fit_channels(stereo.clone(), 1), (1, vec![0.5, 0.5, 0.0]));
        for channels in [2, 6, 8] {
            assert_eq!(fit_channels(stereo.clone(), channels), (2, stereo.clone()));
        }
        assert_eq!(channel_note(2), None);
        assert!(channel_note(1).is_some());
        assert!(channel_note(8).is_some());
    }

    /// A mono chunk steps its ramps once per sample, which is once per
    /// frame, and counts its frames out of the queue as it plays.
    #[test]
    fn a_mono_chunk_ramps_and_counts_by_frame() {
        let queued = Queued::new();
        let (interrupt, transport) = wide_open();
        queued.appended.fetch_add(10, Ordering::Relaxed);
        let chunk = TransitionSource::new(
            rodio::buffer::SamplesBuffer::new(1, RATE, vec![1.0; 10]),
            Arc::clone(&interrupt),
            Arc::clone(&transport),
            Arc::clone(&queued),
            10,
        );
        interrupt.fade_out();
        let played: Vec<_> = chunk.collect();
        assert_eq!(played.len(), 10);
        assert!(played.windows(2).all(|pair| pair[0] > pair[1]));
        assert_eq!(queued.frames(), 0);
    }
}
