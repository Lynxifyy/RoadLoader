//! Audio bridge helpers.
//!
//! The 32-bit `SoundEngine` produces 16-bit PCM. The 64-bit UI mixes in f32, so
//! the companion converts and packs the stream into [`AudioBlock`]s that travel
//! over the shared audio ring. Block size is fixed (4096 bytes) so the audio
//! thread never allocates while mixing.

use crate::ipc::layout::{AudioBlock, AUDIO_DATA_SIZE};

/// Blocks a single [`AudioPacer::tick`] may emit after a stall.
///
/// Replaying a whole backlog would push hundreds of milliseconds of audio into
/// the ring at once, which the UI then plays late. Four periods covers ordinary
/// scheduler jitter; beyond that the pacer resynchronises and counts the skip.
const MAX_CATCH_UP_BLOCKS: u32 = 4;

/// Tone frequency of the demo payload, in hertz.
pub const DEMO_TONE_HZ: f32 = 440.0;

/// Converts interleaved i16 PCM to f32 in `[-1.0, 1.0)`.
pub fn convert_i16_to_f32(input: &[i16], output: &mut [f32]) -> usize {
    let count = input.len().min(output.len());
    for (dst, &src) in output.iter_mut().zip(&input[..count]) {
        *dst = f32::from(src) / 32768.0;
    }
    count
}

/// Converts interleaved f32 PCM to i16, saturating at the endpoints.
pub fn convert_f32_to_i16(input: &[f32], output: &mut [i16]) -> usize {
    let count = input.len().min(output.len());
    for (dst, &src) in output.iter_mut().zip(&input[..count]) {
        let scaled = (src * 32767.0).clamp(-32768.0, 32767.0);
        *dst = scaled as i16;
    }
    count
}

/// Linear resampler for the 44.1 kHz -> 48 kHz device mismatch.
pub fn resample_linear(
    input: &[f32],
    channels: u32,
    from_rate: u32,
    to_rate: u32,
    output: &mut [f32],
) -> usize {
    if channels == 0 || from_rate == 0 || to_rate == 0 || input.is_empty() {
        return 0;
    }
    let frames_in = input.len() / channels as usize;
    if frames_in == 0 {
        return 0;
    }
    let frames_out = ((frames_in as u64 * to_rate as u64) / from_rate as u64) as usize;
    let frames_out = frames_out.min(output.len() / channels as usize);
    if frames_out == 0 {
        return 0;
    }
    for frame in 0..frames_out {
        let source_position = frame as f64 * from_rate as f64 / to_rate as f64;
        let index = source_position.floor() as usize;
        let fraction = (source_position - index as f64) as f32;
        let next = (index + 1).min(frames_in - 1);
        for channel in 0..channels as usize {
            let a = input[index * channels as usize + channel];
            let b = input[next * channels as usize + channel];
            output[frame * channels as usize + channel] = a + (b - a) * fraction;
        }
    }
    frames_out * channels as usize
}

/// Packs f32 frames into a fixed-size audio block.
pub fn pack_block(frames: &[f32], channels: u32, sample_rate: u32) -> AudioBlock {
    let max_samples = AUDIO_DATA_SIZE / core::mem::size_of::<f32>();
    let count = frames.len().min(max_samples);
    let mut block = AudioBlock {
        frames: (count / channels.max(1) as usize) as u32,
        channels,
        sample_rate,
        timestamp_ns: crate::util::monotonic_ns(),
        length: (count * core::mem::size_of::<f32>()) as u32,
        ..Default::default()
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(frames.as_ptr() as *const u8, count * core::mem::size_of::<f32>())
    };
    block.data[..bytes.len()].copy_from_slice(bytes);
    block
}

/// Streaming converter from the game's i16 stereo output to UI-ready f32 blocks.
pub struct AudioBridge {
    sample_rate: u32,
    channels: u32,
    staging: Vec<f32>,
}

impl AudioBridge {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        Self { sample_rate, channels: channels.clamp(1, 8), staging: Vec::with_capacity(4096) }
    }

    /// Adds interleaved i16 samples and returns every complete block.
    pub fn push_i16(&mut self, input: &[i16]) -> Vec<AudioBlock> {
        let start = self.staging.len();
        self.staging.resize(start + input.len(), 0.0);
        convert_i16_to_f32(input, &mut self.staging[start..]);
        self.drain_blocks()
    }

    /// Adds interleaved f32 samples and returns every complete block.
    pub fn push_f32(&mut self, input: &[f32]) -> Vec<AudioBlock> {
        let mut blocks = Vec::new();
        self.push_f32_into(input, &mut blocks);
        blocks
    }

    /// Adds interleaved f32 samples and appends every complete block to `out`.
    ///
    /// The caller owns `out`, so a producer that runs every block period can
    /// keep one buffer for the lifetime of the stream instead of allocating a
    /// `Vec` per block on the audio path.
    pub fn push_f32_into(&mut self, input: &[f32], out: &mut Vec<AudioBlock>) -> usize {
        self.staging.extend_from_slice(input);
        self.drain_into(out)
    }

    fn drain_blocks(&mut self) -> Vec<AudioBlock> {
        let mut blocks = Vec::new();
        self.drain_into(&mut blocks);
        blocks
    }

    fn drain_into(&mut self, out: &mut Vec<AudioBlock>) -> usize {
        let max_samples = self.block_samples();
        let mut emitted = 0;
        while self.staging.len() >= max_samples {
            let block = pack_block(&self.staging[..max_samples], self.channels, self.sample_rate);
            // `drain` shifts the remainder in place; `split_off` would allocate
            // a new buffer for the tail on every block.
            self.staging.drain(..max_samples);
            out.push(block);
            emitted += 1;
        }
        emitted
    }

    /// Samples in one block, rounded down to whole interleaved frames.
    fn block_samples(&self) -> usize {
        (AUDIO_DATA_SIZE / core::mem::size_of::<f32>()) / self.channels as usize
            * self.channels as usize
    }

    pub fn pending_samples(&self) -> usize {
        self.staging.len()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }
}

/// Produces audio blocks on a fixed wall-clock schedule.
///
/// An engine's audio callback runs whether or not anything is audible: it writes
/// silence when the mixer has nothing. Producing only when a sound starts is
/// what starves the 64-bit `AudioTrack` - the track stays in `PLAYSTATE_PLAYING`
/// with an empty buffer and logs an underrun - so the pacer hands the ring one
/// block per elapsed period and counts silence as a block.
pub struct AudioPacer {
    bridge: AudioBridge,
    channels: u32,
    sample_rate: u32,
    /// Samples per block, rounded down to whole interleaved frames.
    block_samples: usize,
    /// Nanoseconds between two blocks.
    period_ns: u64,
    /// Monotonic deadline of the next block; 0 until the first tick.
    next_ns: u64,
    /// Interleaved samples left in the tone being played.
    tone_remaining: usize,
    /// Tone phase in cycles, kept across blocks so a tone does not click.
    phase: f32,
    volume: f32,
    scratch: Vec<f32>,
    blocks: Vec<AudioBlock>,
    produced: u64,
    late_periods: u64,
}

impl AudioPacer {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        let bridge = AudioBridge::new(sample_rate, channels);
        let channels = bridge.channels();
        let sample_rate = bridge.sample_rate();
        let block_samples = bridge.block_samples();
        let frames = block_samples / channels as usize;
        let period_ns = if sample_rate == 0 || frames == 0 {
            0
        } else {
            (frames as u64).saturating_mul(1_000_000_000) / sample_rate as u64
        };
        Self {
            bridge,
            channels,
            sample_rate,
            block_samples,
            period_ns,
            next_ns: 0,
            tone_remaining: 0,
            phase: 0.0,
            volume: 1.0,
            scratch: vec![0.0; block_samples],
            blocks: Vec::with_capacity(MAX_CATCH_UP_BLOCKS as usize),
            produced: 0,
            late_periods: 0,
        }
    }

    /// Queues a demo tone of `frames` stereo frames, mixed into the next blocks.
    pub fn trigger_tone(&mut self, frames: usize) {
        self.tone_remaining = frames.saturating_mul(self.channels as usize);
        self.phase = 0.0;
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Emits every block due at `now_ns` and returns them.
    ///
    /// The deadline advances by exactly one period per block, so a slow tick
    /// does not shift the schedule: audio never drifts against the wall clock.
    pub fn tick(&mut self, now_ns: u64) -> &[AudioBlock] {
        self.blocks.clear();
        if self.period_ns == 0 {
            return &self.blocks;
        }
        if self.next_ns == 0 {
            self.next_ns = now_ns;
        }
        let mut due = 0u32;
        while self.next_ns <= now_ns && due < MAX_CATCH_UP_BLOCKS {
            self.next_ns = self.next_ns.saturating_add(self.period_ns);
            due += 1;
        }
        if self.next_ns <= now_ns {
            // Further behind than the cap allows: resynchronise instead of
            // replaying the backlog, and count the periods that were skipped.
            let skipped = (now_ns - self.next_ns) / self.period_ns + 1;
            self.late_periods += skipped;
            self.next_ns = now_ns.saturating_add(self.period_ns);
        }
        for _ in 0..due {
            self.fill_block();
        }
        &self.blocks
    }

    fn fill_block(&mut self) {
        if self.tone_remaining == 0 {
            self.scratch.fill(0.0);
        } else {
            let step = DEMO_TONE_HZ / self.sample_rate as f32;
            for slot in self.scratch.iter_mut() {
                if self.tone_remaining == 0 {
                    *slot = 0.0;
                    continue;
                }
                self.phase += step;
                if self.phase >= 1.0 {
                    self.phase -= 1.0;
                }
                *slot = (self.phase * core::f32::consts::TAU).sin() * 0.15 * self.volume;
                self.tone_remaining -= 1;
            }
        }
        let before = self.blocks.len();
        self.bridge.push_f32_into(&self.scratch, &mut self.blocks);
        self.produced += (self.blocks.len() - before) as u64;
    }

    /// Blocks emitted since construction.
    pub fn produced(&self) -> u64 {
        self.produced
    }

    /// Periods skipped because a tick arrived too late to cover them.
    pub fn late_periods(&self) -> u64 {
        self.late_periods
    }

    /// Nanoseconds of audio in one block.
    pub fn period_ns(&self) -> u64 {
        self.period_ns
    }

    pub fn block_samples(&self) -> usize {
        self.block_samples
    }

    pub fn channels(&self) -> u32 {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i16_conversion_matches_reference() {
        let input = [0i16, 16384, -16384, 32767, -32768];
        let mut output = [0.0f32; 5];
        assert_eq!(convert_i16_to_f32(&input, &mut output), 5);
        assert_eq!(output[0], 0.0);
        assert_eq!(output[1], 0.5);
        assert_eq!(output[2], -0.5);
        assert!((output[3] - 0.999_969_5).abs() < 1e-6);
        assert_eq!(output[4], -1.0);
    }

    #[test]
    fn f32_conversion_saturates() {
        let input = [0.0f32, 2.0, -2.0, 0.5];
        let mut output = [0i16; 4];
        assert_eq!(convert_f32_to_i16(&input, &mut output), 4);
        assert_eq!(output[0], 0);
        assert_eq!(output[1], 32767);
        assert_eq!(output[2], -32768);
        assert_eq!(output[3], 16383);
    }

    #[test]
    fn resampling_produces_expected_length() {
        let input = vec![0.0f32; 441 * 2];
        let mut output = vec![0.0f32; 960 * 2];
        let written = resample_linear(&input, 2, 44_100, 48_000, &mut output);
        // 441 input frames at 44.1 kHz become 480 frames at 48 kHz.
        assert_eq!(written, 480 * 2);
    }

    #[test]
    fn resampling_interpolates_between_samples() {
        let input = [0.0f32, 1.0];
        let mut output = [0.0f32; 3];
        let written = resample_linear(&input, 1, 2, 3, &mut output);
        assert_eq!(written, 3);
        assert_eq!(output[0], 0.0);
        assert!((output[1] - 2.0 / 3.0).abs() < 1e-6);
        assert_eq!(output[2], 1.0);
    }

    #[test]
    fn blocks_are_fixed_size_and_carry_pcm() {
        let frames = vec![0.25f32; AUDIO_DATA_SIZE / 4];
        let block = pack_block(&frames, 2, 48_000);
        assert_eq!(block.length as usize, AUDIO_DATA_SIZE);
        assert_eq!(block.frames, (AUDIO_DATA_SIZE / 4 / 2) as u32);
        assert_eq!(block.channels, 2);
        assert_eq!(block.sample_rate, 48_000);
        let first = f32::from_le_bytes(block.data[..4].try_into().unwrap());
        assert_eq!(first, 0.25);
    }

    #[test]
    fn bridge_emits_only_complete_blocks() {
        let mut bridge = AudioBridge::new(48_000, 2);
        let blocks = bridge.push_i16(&[1000; 1000]);
        assert!(blocks.is_empty());
        assert_eq!(bridge.pending_samples(), 1000);
        let blocks = bridge.push_i16(&[1000; 4000]);
        // 4064 bytes per block = 1016 f32 samples, rounded down to whole stereo frames.
        assert_eq!(blocks.len(), 4);
        assert_eq!(bridge.pending_samples(), 5000 - 4 * 1016);
    }

    #[test]
    fn pacer_period_matches_the_block_duration() {
        let pacer = AudioPacer::new(48_000, 2);
        // 508 stereo frames per block at 48 kHz is 10.583 ms.
        assert_eq!(pacer.block_samples(), 1016);
        assert_eq!(pacer.period_ns(), 10_583_333);
    }

    #[test]
    fn pacer_emits_one_block_per_period() {
        let mut pacer = AudioPacer::new(48_000, 2);
        let period = pacer.period_ns();
        // A whole second of ticks, 1 ms apart, must yield one second of audio.
        // The block whose deadline is 994.8 ms is due inside the second, so the
        // count is the quotient rounded up: 95 blocks for 94.47 periods.
        let mut emitted = 0usize;
        for step in 0..1000u64 {
            emitted += pacer.tick(step * 1_000_000).len();
        }
        let periods = 1_000_000_000 / period;
        assert_eq!(emitted as u64, periods + 1);
        assert_eq!(emitted, 95);
        assert_eq!(pacer.produced(), emitted as u64);
    }

    #[test]
    fn pacer_does_not_drift_over_a_minute() {
        let mut pacer = AudioPacer::new(48_000, 2);
        let period = pacer.period_ns();
        let mut emitted = 0u64;
        // 60 s of 3 ms ticks: the deadline must not accumulate rounding error.
        for step in 0..20_000u64 {
            emitted += pacer.tick(step * 3_000_000).len() as u64;
        }
        let expected = 60_000_000_000 / period;
        assert!(emitted.abs_diff(expected) <= 1, "emitted={emitted} expected={expected}");
        assert_eq!(pacer.late_periods(), 0);
    }

    #[test]
    fn pacer_emits_silence_when_idle() {
        let mut pacer = AudioPacer::new(48_000, 2);
        let blocks = pacer.tick(1_000_000);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].length as usize, AUDIO_DATA_SIZE);
        assert!(blocks[0].data.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn pacer_mixes_the_tone_and_then_returns_to_silence() {
        let mut pacer = AudioPacer::new(48_000, 2);
        pacer.trigger_tone(508);
        let mut loudest = 0.0f32;
        let mut nonzero_blocks = 0;
        for step in 0..10u64 {
            let blocks = pacer.tick(step * 11_000_000);
            for block in blocks {
                let peak = block
                    .data
                    .chunks_exact(4)
                    .map(|samples| f32::from_le_bytes(samples.try_into().unwrap()).abs())
                    .fold(0.0f32, f32::max);
                if peak > 0.0 {
                    nonzero_blocks += 1;
                }
                loudest = loudest.max(peak);
            }
        }
        // The tone is 508 frames, so it fits inside the first block only.
        assert_eq!(nonzero_blocks, 1);
        assert!(loudest > 0.1 && loudest <= 0.15, "peak={loudest}");
    }

    #[test]
    fn pacer_volume_scales_the_tone() {
        let mut loud = AudioPacer::new(48_000, 2);
        loud.trigger_tone(508);
        let loud_peak = peak_of(loud.tick(1_000_000));

        let mut quiet = AudioPacer::new(48_000, 2);
        quiet.set_volume(0.25);
        quiet.trigger_tone(508);
        let quiet_peak = peak_of(quiet.tick(1_000_000));

        assert!(quiet_peak > 0.0);
        assert!((quiet_peak - loud_peak * 0.25).abs() < 1e-3, "quiet={quiet_peak} loud={loud_peak}");
    }

    #[test]
    fn pacer_resynchronises_instead_of_replaying_a_backlog() {
        let mut pacer = AudioPacer::new(48_000, 2);
        let period = pacer.period_ns();
        pacer.tick(0);
        // A 500 ms stall would be 47 blocks of backlog.
        let blocks = pacer.tick(500_000_000);
        assert_eq!(blocks.len(), MAX_CATCH_UP_BLOCKS as usize);
        assert!(pacer.late_periods() >= 40, "late={}", pacer.late_periods());
        // The schedule resumes at the current time rather than the stale one.
        assert!(pacer.tick(500_000_000 + period).len() == 1);
    }

    fn peak_of(blocks: &[AudioBlock]) -> f32 {
        blocks
            .iter()
            .flat_map(|block| block.data.chunks_exact(4))
            .map(|samples| f32::from_le_bytes(samples.try_into().unwrap()).abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn bridge_handles_zero_channels_safely() {
        let mut bridge = AudioBridge::new(48_000, 0);
        assert_eq!(bridge.channels(), 1);
        let blocks = bridge.push_f32(&[0.0; 10_000]);
        // 1016 f32 samples per block: 9 full blocks, 856 samples left over.
        assert_eq!(blocks.len(), 9);
        assert_eq!(bridge.pending_samples(), 10_000 - 9 * 1016);
    }
}
