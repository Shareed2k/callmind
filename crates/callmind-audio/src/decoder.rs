use crate::buffer::AudioBuffer;
use crate::errors::AudioError;
use crate::opus::{decode_ogg_opus, is_ogg_opus};
use std::fs::File;
use std::io::Cursor;
use std::io::Read;
use std::path::Path;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use tracing::debug;

/// Pure-Rust audio decoder powered by Symphonia.
pub struct AudioDecoder;

impl AudioDecoder {
    /// Decode an audio file from a local filesystem path.
    pub fn decode_file<P: AsRef<Path>>(path: P) -> Result<AudioBuffer, AudioError> {
        let p = path.as_ref();
        let file = File::open(p).map_err(|e| AudioError::Io {
            path: p.to_path_buf(),
            source: e,
        })?;

        // Sniff the container before handing it to Symphonia: it demuxes OGG but
        // cannot decode Opus, so those streams need the dedicated path.
        let mut file = file;
        let mut prefix = [0u8; 1024];
        let read = file.read(&mut prefix).map_err(|e| AudioError::Io {
            path: p.to_path_buf(),
            source: e,
        })?;
        if is_ogg_opus(&prefix[..read]) {
            let bytes = std::fs::read(p).map_err(|e| AudioError::Io {
                path: p.to_path_buf(),
                source: e,
            })?;
            return decode_ogg_opus(&bytes);
        }
        let file = File::open(p).map_err(|e| AudioError::Io {
            path: p.to_path_buf(),
            source: e,
        })?;

        let mut hint = Hint::new();
        if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }

        Self::decode_source(Box::new(file), hint)
    }

    /// Decode audio from raw in-memory bytes.
    pub fn decode_bytes(
        data: &[u8],
        extension_hint: Option<&str>,
    ) -> Result<AudioBuffer, AudioError> {
        if data.is_empty() {
            return Err(AudioError::EmptyAudio);
        }

        if is_ogg_opus(data) {
            return decode_ogg_opus(data);
        }

        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext);
        }

        let cursor = Cursor::new(data.to_vec());
        Self::decode_source(Box::new(cursor), hint)
    }

    /// Decode an audio media source stream into an `AudioBuffer`.
    /// The audio track's id and codec parameters.
    ///
    /// Both are copied out of the reader's borrow so it can be advanced
    /// afterwards, and `codec_params` is an `Option<CodecParameters>` enum in
    /// symphonia 0.6: a track whose codec could not be determined is unplayable
    /// rather than silently empty.
    fn audio_track(
        reader: &dyn symphonia::core::formats::FormatReader,
    ) -> Result<(u32, AudioCodecParameters), AudioError> {
        let track = reader
            .default_track(TrackType::Audio)
            .or_else(|| {
                reader
                    .tracks()
                    .iter()
                    .find(|t| matches!(t.codec_params, Some(CodecParameters::Audio(_))))
            })
            .ok_or_else(|| {
                AudioError::UnsupportedFormat("No audio track found in media stream".into())
            })?;

        let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
            return Err(AudioError::UnsupportedFormat(
                "Audio track carries no codec parameters".into(),
            ));
        };

        Ok((track.id, params.clone()))
    }

    fn decode_source(source: Box<dyn MediaSource>, hint: Hint) -> Result<AudioBuffer, AudioError> {
        let mss = MediaSourceStream::new(source, Default::default());

        // Gapless playback is a decoder option in symphonia 0.6 rather than a
        // format one, and it defaults to on. It is what drops an Opus encoder's
        // pre-skip, which keeps word timestamps aligned with the audio.
        let mut reader = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| AudioError::UnsupportedFormat(e.to_string()))?;

        // Copied out of the borrow so the reader can be advanced below.
        let (track_id, params) = Self::audio_track(reader.as_ref())?;

        let mut decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| AudioError::Decode(e.to_string()))?;

        let mut sample_rate = params.sample_rate.unwrap_or(16000);
        let mut channels = params.channels.as_ref().map_or(1, |c| c.count() as u16);

        let mut samples_out: Vec<f32> = Vec::new();
        let mut interleaved: Vec<f32> = Vec::new();

        loop {
            let packet = match reader.next_packet() {
                // End of stream is `Ok(None)` now, not an unexpected-EOF error.
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(SymphoniaError::ResetRequired) => continue,
                Err(e) => {
                    debug!("End of audio stream or decode termination: {e}");
                    break;
                }
            };

            if packet.track_id != track_id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    sample_rate = audio_buf.spec().rate();
                    channels = audio_buf.spec().channels().count() as u16;
                    audio_buf.copy_to_vec_interleaved(&mut interleaved);
                    samples_out.extend_from_slice(&interleaved);
                }
                Err(SymphoniaError::DecodeError(e)) => {
                    debug!("Skipping corrupt audio frame: {e}");
                    continue;
                }
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(e) => {
                    return Err(AudioError::Decode(format!("Fatal decode error: {e}")));
                }
            }
        }

        if samples_out.is_empty() {
            return Err(AudioError::EmptyAudio);
        }

        Ok(AudioBuffer::new(sample_rate, channels, samples_out))
    }
}

/// Track metadata read without decoding any audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioMetadata {
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_ms: u64,
}

impl AudioDecoder {
    /// Read duration, channel count and sample rate from the container headers.
    ///
    /// Batch import used to call `decode_file` for this, decoding every packet of
    /// every file to learn three numbers — a 43-minute call materialises ~494 MB
    /// of f32 samples on the way. Symphonia exposes the track's codec parameters
    /// straight from the header, so no packet is decoded here.
    ///
    /// Returns `None` when the container does not declare a frame count (some
    /// streamed OGG files), leaving the caller to fall back to a full decode.
    pub fn read_metadata<P: AsRef<Path>>(path: P) -> Result<Option<AudioMetadata>, AudioError> {
        let p = path.as_ref();
        let file = File::open(p).map_err(|e| AudioError::Io {
            path: p.to_path_buf(),
            source: e,
        })?;

        let mut hint = Hint::new();
        if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
            hint.with_extension(ext);
        }

        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let reader = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| AudioError::UnsupportedFormat(e.to_string()))?;

        let Some(track) = reader.default_track(TrackType::Audio).or_else(|| {
            reader
                .tracks()
                .iter()
                .find(|t| matches!(t.codec_params, Some(CodecParameters::Audio(_))))
        }) else {
            return Ok(None);
        };

        let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
            return Ok(None);
        };
        let Some(sample_rate) = params.sample_rate else {
            return Ok(None);
        };
        let channels = params.channels.as_ref().map_or(1, |c| c.count() as u16);

        // The frame count moved from the codec parameters onto the track in
        // symphonia 0.6, and it now excludes delay and padding -- which is what
        // a duration should count. It is per channel, so duration does not
        // depend on the channel count.
        let Some(frames) = track.num_frames else {
            return Ok(None);
        };
        let duration_ms = (frames.saturating_mul(1000)) / u64::from(sample_rate.max(1));

        Ok(Some(AudioMetadata {
            sample_rate,
            channels: channels.max(1),
            duration_ms,
        }))
    }
}
