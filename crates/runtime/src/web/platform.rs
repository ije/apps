use anyhow::{Context, Result, bail, ensure};
use base64::{
    Engine, alphabet,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

enum Codec {
    Encode(Compress),
    Decode(Decompress),
}

pub struct State {
    origin: Instant,
    epoch: f64,
    next_id: u64,
    codecs: HashMap<u64, (Codec, bool)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
            epoch: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs_f64()
                * 1000.,
            next_id: 0,
            codecs: HashMap::new(),
        }
    }
}

impl State {
    pub fn call(&mut self, op: &str, args: &Value) -> Result<Value> {
        match op {
            "time" => Ok(
                json!({"version": crate::VERSION, "platform": if cfg!(target_os = "macos") { "MacIntel".to_owned() } else if cfg!(target_os = "windows") { "Win32".to_owned() } else { format!("Linux {}", std::env::consts::ARCH) }, "origin": self.epoch, "now": self.origin.elapsed().as_secs_f64() * 1000.}),
            ),
            "base64.encode" => {
                let text = args[0].as_str().context("Expected binary string")?;
                let bytes = text
                    .chars()
                    .map(|c| u8::try_from(c as u32))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(json!(
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                ))
            }
            "base64.decode" => {
                let text = args[0].as_str().context("Expected Base64 string")?;
                let input: Vec<_> = text
                    .bytes()
                    .filter(|b| !matches!(b, b'\t' | b'\n' | 0x0c | b'\r' | b' '))
                    .collect();
                let engine = GeneralPurpose::new(
                    &alphabet::STANDARD,
                    GeneralPurposeConfig::new()
                        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
                        .with_decode_allow_trailing_bits(true),
                );
                let bytes = engine.decode(input)?;
                Ok(json!(bytes.into_iter().map(char::from).collect::<String>()))
            }
            "compression.new" => {
                let format = args[0].as_str().context("Expected compression format")?;
                ensure!(
                    matches!(format, "gzip" | "deflate" | "deflate-raw"),
                    "Invalid compression format"
                );
                let codec = if args[1] == true {
                    Codec::Decode(if format == "gzip" {
                        Decompress::new_gzip(15)
                    } else {
                        Decompress::new(format == "deflate")
                    })
                } else {
                    Codec::Encode(if format == "gzip" {
                        Compress::new_gzip(Compression::default(), 15)
                    } else {
                        Compress::new(Compression::default(), format == "deflate")
                    })
                };
                self.next_id += 1;
                self.codecs.insert(self.next_id, (codec, false));
                Ok(json!(self.next_id))
            }
            "compression.write" => {
                let id = args[0].as_u64().context("Expected compression ID")?;
                let bytes = crate::node::bytes(&args[1])?;
                let finish = args[2] == true;
                let (codec, ended) = self
                    .codecs
                    .get_mut(&id)
                    .context("Compression stream is closed")?;
                if *ended {
                    ensure!(bytes.is_empty(), "Trailing compressed data");
                    return Ok(json!({"read": 0, "bytes": "", "done": true}));
                }
                let mut output = vec![0; 65536];
                let (status, read, written) = match codec {
                    Codec::Encode(encoder) => {
                        let input = encoder.total_in();
                        let produced = encoder.total_out();
                        let status = encoder.compress(
                            bytes,
                            &mut output,
                            if finish {
                                FlushCompress::Finish
                            } else {
                                FlushCompress::None
                            },
                        )?;
                        (
                            status,
                            encoder.total_in() - input,
                            encoder.total_out() - produced,
                        )
                    }
                    Codec::Decode(decoder) => {
                        let input = decoder.total_in();
                        let produced = decoder.total_out();
                        let status = decoder.decompress(
                            bytes,
                            &mut output,
                            if finish {
                                FlushDecompress::Finish
                            } else {
                                FlushDecompress::None
                            },
                        )?;
                        (
                            status,
                            decoder.total_in() - input,
                            decoder.total_out() - produced,
                        )
                    }
                };
                *ended = status == Status::StreamEnd;
                ensure!(
                    !*ended || read as usize == bytes.len(),
                    "Trailing compressed data"
                );
                ensure!(
                    !finish || *ended || read > 0 || written > 0,
                    "Incomplete compressed data"
                );
                output.truncate(written as usize);
                Ok(json!({"read": read, "bytes": crate::jsc::payload::write(output), "done": *ended}))
            }
            "compression.close" => {
                self.codecs
                    .remove(&args[0].as_u64().context("Expected compression ID")?);
                Ok(Value::Null)
            }
            _ => bail!("Unknown platform operation: {op}"),
        }
    }
}
