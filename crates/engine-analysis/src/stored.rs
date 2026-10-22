//! One analyzer's result as a library blob: small versioned JSON.
//!
//! ```json
//! {"format":"djplayer-analysis","v":1,
//!  "tempo":{"bpm":128.0,"confidence":0.9},
//!  "beatgrid":{"beats":[[0.12,1],[0.589,2]],"confidence":null},
//!  "key":{"tonic":9,"mode":"minor","confidence":0.7}}
//! ```
//!
//! Absent features are `null`. Times are seconds on the gapless-trimmed timeline, as the
//! analyzers produce them. The analyzer's name, version and parameters live in the record
//! that points at the blob.

use crate::model::{
    Analysed, Analysis, AnalyzerId, Beat, Beatgrid, Key, Mode, PitchClass, Provenance, Tempo,
};
use serde_json::{Value, json};

const FORMAT: &str = "djplayer-analysis";
const VERSION: u64 = 1;

fn confidence(c: Option<f32>) -> Value {
    c.map_or(Value::Null, |c| json!(c))
}

pub fn encode(a: &Analysis) -> Vec<u8> {
    let tempo = a.tempo.as_ref().map_or(
        Value::Null,
        |t| json!({ "bpm": t.value.bpm, "confidence": confidence(t.confidence) }),
    );
    let beatgrid = a.beatgrid.as_ref().map_or(Value::Null, |g| {
        let beats: Vec<Value> = g
            .value
            .beats
            .iter()
            .map(|b| json!([b.time, b.bar_pos]))
            .collect();
        json!({ "beats": beats, "confidence": confidence(g.confidence) })
    });
    let key = a.key.as_ref().map_or(Value::Null, |k| {
        let mode = match k.value.mode {
            Mode::Major => "major",
            Mode::Minor => "minor",
        };
        json!({
            "tonic": k.value.tonic.index(),
            "mode": mode,
            "confidence": confidence(k.confidence),
        })
    });
    let v = json!({
        "format": FORMAT,
        "v": VERSION,
        "tempo": tempo,
        "beatgrid": beatgrid,
        "key": key,
    });
    v.to_string().into_bytes()
}

fn wrap<T>(value: T, c: &Value, id: &AnalyzerId) -> Option<Analysed<T>> {
    Some(Analysed {
        value,
        confidence: c
            .get("confidence")
            .and_then(Value::as_f64)
            .map(|c| c as f32),
        provenance: Provenance::Analyzer(id.clone()),
    })
}

/// The result back, attributed to `id`; `None` if the blob is not one this version reads.
pub fn decode(bytes: &[u8], id: &AnalyzerId) -> Option<Analysis> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    if v.get("format")?.as_str()? != FORMAT || v.get("v")?.as_u64()? != VERSION {
        return None;
    }
    let tempo = match v.get("tempo")? {
        Value::Null => None,
        t => wrap(
            Tempo {
                bpm: t.get("bpm")?.as_f64()?,
            },
            t,
            id,
        ),
    };
    let beatgrid = match v.get("beatgrid")? {
        Value::Null => None,
        g => {
            let beats = g
                .get("beats")?
                .as_array()?
                .iter()
                .map(|b| {
                    Some(Beat {
                        time: b.get(0)?.as_f64()?,
                        bar_pos: u8::try_from(b.get(1)?.as_u64()?).ok()?,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            wrap(Beatgrid { beats }, g, id)
        }
    };
    let key = match v.get("key")? {
        Value::Null => None,
        k => {
            let mode = match k.get("mode")?.as_str()? {
                "major" => Mode::Major,
                "minor" => Mode::Minor,
                _ => return None,
            };
            let tonic = PitchClass::new(i32::try_from(k.get("tonic")?.as_u64()?).ok()?);
            wrap(Key { tonic, mode }, k, id)
        }
    };
    Some(Analysis {
        tempo,
        beatgrid,
        key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_analysis_round_trips() {
        let id = AnalyzerId {
            name: "gridfit".into(),
            version: "0.1.0".into(),
            params: "p".into(),
        };
        let p = Provenance::Analyzer(id.clone());
        let a = Analysis {
            tempo: Some(Analysed {
                value: Tempo { bpm: 123.456 },
                confidence: Some(0.75),
                provenance: p.clone(),
            }),
            beatgrid: Some(Analysed {
                value: Beatgrid::constant(0.1 + 0.2, 123.456, 10.0),
                confidence: None,
                provenance: p.clone(),
            }),
            key: Some(Analysed {
                value: Key::minor(9),
                confidence: Some(0.5),
                provenance: p,
            }),
        };
        assert_eq!(decode(&encode(&a), &id), Some(a));
        let empty = Analysis::default();
        assert_eq!(decode(&encode(&empty), &id), Some(empty));
        assert_eq!(decode(b"{\"format\":\"other\"}", &id), None);
    }
}
