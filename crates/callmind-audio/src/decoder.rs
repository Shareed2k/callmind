use crate::buffer::AudioBuffer;
use crate::errors::AudioError;
use std::fs::File;
use std::io::Cursor;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
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

        let mut hint = Hint::new();
        if let Some(ext) = extension_hint {
            hint.with_extension(ext);
        }

        let cursor = Cursor::new(data.to_vec());
        Self::decode_source(Box::new(cursor), hint)
    }

    /// Decode an audio media source stream into an `AudioBuffer`.
    fn decode_source(source: Box<dyn MediaSource>, hint: Hint) -> Result<AudioBuffer, AudioError> {
        let mss = MediaSourceStream::new(source, Default::default());

        let fmt_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let meta_opts = MetadataOptions::default();

        let mut probed = symphonia::default::get_probe()
            .format(&hint, mss, &fmt_opts, &meta_opts)
            .map_err(|e| AudioError::UnsupportedFormat(e.to_string()))?;

        let track = probed
            .format
            .default_track()
            .or_else(|| probed.format.tracks().first())
            .ok_or_else(|| {
                AudioError::UnsupportedFormat("No audio track found in media stream".into())
            })?;

        let dec_opts = DecoderOptions::default();
        let mut decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &dec_opts)
            .map_err(|e| AudioError::Decode(e.to_string()))?;

        let track_id = track.id;
        let mut sample_rate = track.codec_params.sample_rate.unwrap_or(16000);
        let mut channels = track.codec_params.channels.map_or(1, |c| c.count() as u16);

        let mut samples_out: Vec<f32> = Vec::new();
        let mut sample_buf: Option<SampleBuffer<f32>> = None;

        loop {
            let packet = match probed.format.next_packet() {
                Ok(packet) => packet,
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    break;
                }
                Err(SymphoniaError::ResetRequired) => {
                    continue;
                }
                Err(e) => {
                    debug!("End of audio stream or decode termination: {e}");
                    break;
                }
            };

            if packet.track_id() != track_id {
                continue;
            }

            match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    let spec = *audio_buf.spec();
                    sample_rate = spec.rate;
                    channels = spec.channels.count() as u16;

                    if sample_buf.is_none() {
                        sample_buf = Some(SampleBuffer::new(audio_buf.capacity() as u64, spec));
                    }

                    if let Some(buf) = sample_buf.as_mut() {
                        buf.copy_interleaved_ref(audio_buf);
                        samples_out.extend_from_slice(buf.samples());
                    }
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
