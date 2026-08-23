use crate::buffer::AudioBuffer;
use crate::errors::AudioError;
use ogg::PacketReader;
use std::io::Cursor;
use tracing::debug;

/// Opus is always decoded at 48 kHz, its native rate and the one WhatsApp and
/// Telegram voice notes are encoded at. The pipeline resamples to 16 kHz later.
const OPUS_DECODE_RATE: u32 = 48_000;

/// Largest Opus frame is 120 ms, i.e. 5760 samples per channel at 48 kHz.
const MAX_FRAME_SAMPLES: usize = 5760;

/// Detect OGG/Opus from the container bytes rather than the file extension.
///
/// Symphonia demuxes the OGG container but has no Opus decoder — there is no
/// `symphonia-codec-opus` crate — so these streams have to be routed elsewhere.
#[must_use]
pub fn is_ogg_opus(prefix: &[u8]) -> bool {
    prefix.starts_with(b"OggS") && prefix.windows(8).any(|w| w == b"OpusHead")
}

/// Decode an OGG-encapsulated Opus stream to interleaved f32 at 48 kHz.
pub fn decode_ogg_opus(data: &[u8]) -> Result<AudioBuffer, AudioError> {
    if data.is_empty() {
        return Err(AudioError::EmptyAudio);
    }

    let mut reader = PacketReader::new(Cursor::new(data));
    let mut decoder: Option<opus::Decoder> = None;
    let mut channels: usize = 1;
    let mut pre_skip: usize = 0;
    let mut samples: Vec<f32> = Vec::new();

    loop {
        let packet = match reader.read_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(e) => return Err(AudioError::Decode(format!("OGG read error: {e}"))),
        };
        let payload = packet.data.as_slice();

        // Identification header: magic(8) version(1) channels(1) pre_skip(2 LE) ...
        if payload.starts_with(b"OpusHead") {
            if payload.len() < 12 {
                return Err(AudioError::Decode(
                    "truncated OpusHead identification header".into(),
                ));
            }
            channels = usize::from(payload[9]).max(1);
            pre_skip = usize::from(u16::from_le_bytes([payload[10], payload[11]]));
            let layout = if channels >= 2 {
                opus::Channels::Stereo
            } else {
                opus::Channels::Mono
            };
            decoder = Some(
                opus::Decoder::new(OPUS_DECODE_RATE, layout)
                    .map_err(|e| AudioError::Decode(format!("Opus decoder init failed: {e}")))?,
            );
            continue;
        }

        // Comment header carries only metadata.
        if payload.starts_with(b"OpusTags") {
            continue;
        }

        let Some(decoder) = decoder.as_mut() else {
            // Audio before the identification header: not a usable Opus stream.
            continue;
        };

        let mut frame = vec![0f32; MAX_FRAME_SAMPLES * channels];
        match decoder.decode_float(payload, &mut frame, false) {
            // `decode_float` reports samples *per channel*.
            Ok(per_channel) => samples.extend_from_slice(&frame[..per_channel * channels]),
            Err(e) => {
                // A single corrupt packet should not discard the whole recording.
                debug!("Skipping undecodable Opus packet: {e}");
            }
        }
    }

    if decoder.is_none() {
        return Err(AudioError::UnsupportedFormat(
            "OGG stream contains no Opus identification header".into(),
        ));
    }
    if samples.is_empty() {
        return Err(AudioError::EmptyAudio);
    }

    // The encoder's priming samples are not real audio; dropping them keeps
    // timestamps aligned with the original recording.
    let skip = (pre_skip * channels).min(samples.len());
    samples.drain(..skip);

    let channels = u16::try_from(channels).unwrap_or(1);
    Ok(AudioBuffer::new(OPUS_DECODE_RATE, channels, samples))
}
