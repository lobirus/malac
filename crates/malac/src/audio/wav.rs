//! 16-bit PCM WAV: what dictation sends, and what test recordings and some
//! recorders' streams are.

use anyhow::{Context, Result, bail};

/// 16-bit samples, interleaved when there are several channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

/// A mono 16-bit WAV file of `samples` at `rate`.
pub fn encode(samples: &[i16], rate: u32) -> Vec<u8> {
    let data = (samples.len() * 2) as u32;
    let mut wav = Vec::with_capacity(44 + data as usize);
    wav.extend(b"RIFF");
    wav.extend((36 + data).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes()); // PCM
    wav.extend(1u16.to_le_bytes()); // mono
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(data.to_le_bytes());
    for sample in samples {
        wav.extend(sample.to_le_bytes());
    }
    wav
}

/// Reads a 16-bit PCM WAV file, skipping chunks it does not need.
pub fn decode(bytes: &[u8]) -> Result<Pcm> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("not a WAV file");
    }
    let mut format = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let start = at + 8;
        // A recorder writing to a pipe cannot know the data's size.
        let end = match id == b"data" && (size == 0 || size == u32::MAX as usize) {
            true => bytes.len(),
            false => start.saturating_add(size).min(bytes.len()),
        };
        let body = &bytes[start..end];
        match id {
            b"fmt " => {
                if body.len() < 16 {
                    bail!("the WAV file's format is cut short");
                }
                let word = |i: usize| u16::from_le_bytes([body[i], body[i + 1]]);
                let mut tag = word(0);
                if tag == 0xFFFE && body.len() >= 26 {
                    tag = word(24);
                }
                if tag != 1 || word(14) != 16 {
                    bail!("only 16-bit PCM WAV files can be read");
                }
                let rate = u32::from_le_bytes(body[4..8].try_into().unwrap());
                format = Some((rate, word(2).max(1)));
            }
            b"data" => {
                let (rate, channels) = format.context("the WAV file has no format chunk")?;
                return Ok(Pcm {
                    rate,
                    channels,
                    samples: samples(body),
                });
            }
            _ => {}
        }
        at = end + (size & 1);
    }
    bail!("the WAV file has no data")
}

/// Where the samples start in a stream that may begin with a WAV header;
/// `None` while the header is incomplete.
pub fn data_offset(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 4 {
        return None;
    }
    if &bytes[..4] != b"RIFF" {
        return Some(0);
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        if &bytes[at..at + 4] == b"data" {
            return Some(at + 8);
        }
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        at += 8 + size + (size & 1);
    }
    None
}

/// Little-endian 16-bit samples; a trailing odd byte is dropped.
pub fn samples(bytes: &[u8]) -> Vec<i16> {
    bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_skips_other_chunks() {
        let samples = vec![0, 1, -1, i16::MAX, i16::MIN, 1234];
        let wav = encode(&samples, 16_000);
        assert_eq!(wav.len(), 44 + 12);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36 + 12);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[28..32].try_into().unwrap()), 32_000);
        let pcm = decode(&wav).unwrap();
        assert_eq!((pcm.rate, pcm.channels), (16_000, 1));
        assert_eq!(pcm.samples, samples);

        // `say` and others put chunks such as FLLR or LIST before the data.
        let mut padded = wav[..36].to_vec();
        padded.extend(b"LIST");
        padded.extend(3u32.to_le_bytes());
        padded.extend([1, 2, 3, 0]);
        padded.extend(&wav[36..]);
        assert_eq!(decode(&padded).unwrap().samples, samples);
        assert_eq!(data_offset(&padded), Some(padded.len() - 12));
    }

    #[test]
    fn reads_streams_and_refuses_other_formats() {
        let mut streamed = encode(&[5, 6, 7], 24_000);
        streamed[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&streamed).unwrap().samples, vec![5, 6, 7]);
        assert_eq!(data_offset(&streamed), Some(44));
        assert_eq!(data_offset(&[1, 0, 2, 0]), Some(0));
        assert_eq!(data_offset(b"RIFF\0\0"), None);

        let mut float = encode(&[1], 16_000);
        float[20] = 3;
        assert!(decode(&float).is_err());
        assert!(decode(b"ID3 not a wav file").is_err());
    }
}
