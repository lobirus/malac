//! Mono downmixing and a small streaming resampler: enough for speech,
//! which dictation sends at 16 kHz and talk mode at 24 kHz.

use std::collections::VecDeque;

/// Averages each frame of `channels` interleaved samples into one.
pub fn downmix(interleaved: &[i16], channels: u16) -> Vec<i16> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks(channels as usize)
        .map(|frame| {
            let sum: i32 = frame.iter().map(|s| i32::from(*s)).sum();
            (sum / frame.len() as i32) as i16
        })
        .collect()
}

/// Changes the rate of a mono stream fed in chunks of any size, by linear
/// interpolation. Lowering the rate first averages over the samples each
/// output sample stands for, which keeps most of the aliasing out.
pub struct Resampler {
    from: u32,
    to: u32,
    /// Where the next output sample falls, in input samples after `last`.
    position: f64,
    /// The previous chunk's final (smoothed) sample.
    last: Option<f32>,
    window: usize,
    history: VecDeque<f32>,
    sum: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        let (from, to) = (from.max(1), to.max(1));
        Self {
            from,
            to,
            position: 0.0,
            last: None,
            window: if from > to { (from / to) as usize } else { 1 },
            history: VecDeque::new(),
            sum: 0.0,
        }
    }

    pub fn process(&mut self, input: &[i16]) -> Vec<i16> {
        if self.from == self.to {
            return input.to_vec();
        }
        let mut points: Vec<f32> = Vec::with_capacity(input.len() + 1);
        points.extend(self.last);
        for &sample in input {
            let sample = f32::from(sample);
            if self.window > 1 {
                self.history.push_back(sample);
                self.sum += f64::from(sample);
                if self.history.len() > self.window {
                    self.sum -= f64::from(self.history.pop_front().unwrap_or_default());
                }
                points.push((self.sum / self.history.len() as f64) as f32);
            } else {
                points.push(sample);
            }
        }
        let Some(&final_point) = points.last() else {
            return Vec::new();
        };
        let step = f64::from(self.from) / f64::from(self.to);
        let span = (points.len() - 1) as f64;
        let mut out = Vec::with_capacity((input.len() as f64 / step) as usize + 1);
        while self.position < span {
            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            let value = points[index] + (points[index + 1] - points[index]) * fraction;
            out.push(value.round().clamp(-32768.0, 32767.0) as i16);
            self.position += step;
        }
        self.position -= span;
        self.last = Some(final_point);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, hertz: f32, seconds: f32) -> Vec<i16> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                ((t * hertz * std::f32::consts::TAU).sin() * 12_000.0) as i16
            })
            .collect()
    }

    /// Upward zero crossings, which a resampled tone keeps.
    fn crossings(samples: &[i16]) -> usize {
        samples.windows(2).filter(|w| w[0] < 0 && w[1] >= 0).count()
    }

    #[test]
    fn downmixes_frames() {
        assert_eq!(downmix(&[100, 300, -50, 50], 2), vec![200, 0]);
        assert_eq!(downmix(&[1, 2, 3], 1), vec![1, 2, 3]);
        assert_eq!(downmix(&[30, 30, 30, 9], 3), vec![30, 9]);
    }

    #[test]
    fn keeps_the_pitch_and_the_length() {
        let input = tone(48_000, 440.0, 1.0);
        let down = Resampler::new(48_000, 16_000).process(&input);
        assert!((15_990..=16_010).contains(&down.len()), "{}", down.len());
        assert!(
            (438..=441).contains(&crossings(&down)),
            "{}",
            crossings(&down)
        );
        let peak = down.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 10_000, "a 440 Hz tone passes the smoothing: {peak}");

        let up = Resampler::new(24_000, 48_000).process(&tone(24_000, 300.0, 0.5));
        assert!((23_990..=24_010).contains(&up.len()), "{}", up.len());
        assert!((148..=151).contains(&crossings(&up)), "{}", crossings(&up));

        let same = tone(16_000, 200.0, 0.1);
        assert_eq!(Resampler::new(16_000, 16_000).process(&same), same);
    }

    #[test]
    fn chunks_add_up_to_the_whole() {
        for (from, to) in [(48_000, 16_000), (24_000, 48_000)] {
            let input = tone(from, 523.0, 0.3);
            let whole = Resampler::new(from, to).process(&input);
            let mut resampler = Resampler::new(from, to);
            let mut pieces = Vec::new();
            for chunk in input.chunks(441).chain([&[][..]]) {
                pieces.extend(resampler.process(chunk));
            }
            assert_eq!(pieces, whole, "{from} to {to}");
        }
        let mut single = Resampler::new(8_000, 16_000);
        let one: Vec<i16> = [10, 20, 30]
            .iter()
            .flat_map(|s| single.process(&[*s]))
            .collect();
        assert_eq!(one, vec![10, 15, 20, 25]);
    }
}
