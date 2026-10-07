use std::fmt;

use anyhow::Context;
use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Number, Value};
use serde_toon::DecodeOptions;

use super::adapter::{OpenedSource, ProbeResult, SourceAdapter};
use super::json::open_decoded_toon;
use super::source::{read_source, InputSource};
use super::{InputFormat, OpenOptions};

#[derive(Debug, Clone, Copy)]
pub struct ToonAdapter;

impl SourceAdapter for ToonAdapter {
    fn format(&self) -> InputFormat {
        InputFormat::Toon
    }

    fn probe(&self, source: &InputSource, _sample: &[u8]) -> ProbeResult {
        match source {
            InputSource::Path(path)
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("toon")) =>
            {
                ProbeResult::Strong
            }
            _ => ProbeResult::NoMatch,
        }
    }

    fn open(&self, source: InputSource, options: &OpenOptions) -> anyhow::Result<OpenedSource> {
        options.validate()?;
        if options.delimited.encoding.is_some()
            || options.delimited.delimiter.is_some()
            || options.delimited.quoting.is_some()
            || options.delimited.quote_char != b'"'
        {
            anyhow::bail!(
                "encoding, delimiter, quoting, and quote-character options cannot be used with TOON input"
            );
        }
        let display_name = source.display_name();
        // Full byte validation is required even for preview and source-limited tables.
        let bytes = read_source(&source).context("reading TOON input")?;
        let CheckedValue(root) =
            serde_toon::from_slice_with_options(&bytes, DecodeOptions::strict())
                .context("decoding TOON input")?;
        open_decoded_toon(&root, display_name, options).context("projecting TOON input")
    }
}

// serde_json::Value converts non-finite f64 visits to null. A TOON token such
// as 1e999 would otherwise silently turn into a null cell; reject it instead.
// This visitor constructs the same ordered serde_json::Value tree directly.
struct CheckedValue(Value);

impl<'de> Deserialize<'de> for CheckedValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct CheckedVisitor;

        impl<'de> Visitor<'de> for CheckedVisitor {
            type Value = CheckedValue;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a finite JSON-shaped TOON value")
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(CheckedValue(Value::Null))
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                self.visit_unit()
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(CheckedValue(Value::Bool(value)))
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(CheckedValue(Value::Number(value.into())))
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(CheckedValue(Value::Number(value.into())))
            }

            fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
                Number::from_f64(value)
                    .map(Value::Number)
                    .map(CheckedValue)
                    .ok_or_else(|| E::custom("TOON number is outside the finite f64 range"))
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(CheckedValue(Value::String(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(CheckedValue(Value::String(value)))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(CheckedValue(value)) = seq.next_element()? {
                    values.push(value);
                }
                Ok(CheckedValue(Value::Array(values)))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, CheckedValue(value))) = map.next_entry()? {
                    values.insert(key, value);
                }
                Ok(CheckedValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(CheckedVisitor)
    }
}
