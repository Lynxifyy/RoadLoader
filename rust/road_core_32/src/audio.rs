//! Audio bridge helpers.
//!
//! The 32-bit `SoundEngine` produces 16-bit PCM. The 64-bit UI mixes in f32, so
//! the companion converts and packs the stream into [`AudioBlock`]s that travel
//! over the shared audio ring. Block size is fixed (4096 bytes) so the audio
//! thread never allocates while mixing.

use crate::ipc::layout::{AudioBlock, AUDIO_DATA_SIZE};

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
        self.staging.extend_from_slice(input);
        self.drain_blocks()
    }

    fn drain_blocks(&mut self) -> Vec<AudioBlock> {
        let max_samples = (AUDIO_DATA_SIZE / core::mem::size_of::<f32>())
            / self.channels as usize
            * self.channels as usize;
        let mut blocks = Vec::new();
        while self.staging.len() >= max_samples {
            let rest = self.staging.split_off(max_samples);
            let block = pack_block(&self.staging, self.channels, self.sample_rate);
            self.staging = rest;
            blocks.push(block);
        }
        blocks
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
    fn bridge_handles_zero_channels_safely() {
        let mut bridge = AudioBridge::new(48_000, 0);
        assert_eq!(bridge.channels(), 1);
        let blocks = bridge.push_f32(&[0.0; 10_000]);
        // 1016 f32 samples per block: 9 full blocks, 856 samples left over.
        assert_eq!(blocks.len(), 9);
        assert_eq!(bridge.pending_samples(), 10_000 - 9 * 1016);
    }
}
