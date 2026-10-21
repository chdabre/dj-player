//! [Essentia](https://essentia.upf.edu) backends, run in a Python worker process
//! (`essentia/bridge.py`), since Essentia is a large C++ library whose prebuilt form is the
//! `essentia-tensorflow` wheel.
//!
//! - [`Variant::Dsp`] (`essentia`): `RhythmExtractor2013` (multifeature) for tempo and beats,
//!   `KeyExtractor` with the EDM profile for key. It finds beats but not bars.
//! - [`Variant::TempoCnn`] (`essentia-tempocnn`): the TempoCNN model (`deeptemp-k16-3`) for
//!   tempo only, in whole BPM. Essentia has no beat or key model.
//!
//! Configuration comes from the environment: `ESSENTIA_PYTHON` (a Python with
//! `essentia-tensorflow`) and, for TempoCNN, `ESSENTIA_TEMPOCNN` (path to the `.pb`). Both
//! Essentia and that model are AGPL-3.0; neither is bundled.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::Value;

use crate::analyzer::{AnalysisError, Analyzer, CancelFlag};
use crate::model::{
    Analysed, Analysis, AnalyzerId, Beatgrid, Feature, FeatureSet, Key, Provenance, Tempo,
    TrackAudio,
};

const BRIDGE: &str = include_str!("../essentia/bridge.py");
/// Essentia's rhythm and key extractors are designed for 44.1 kHz.
const ANALYSIS_RATE: u32 = 44_100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Dsp,
    TempoCnn,
}

pub struct Essentia {
    variant: Variant,
    version: String,
    worker: Worker,
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Essentia {
    /// Start a worker with this Python. `tempocnn_model` is required for [`Variant::TempoCnn`].
    pub fn start(
        variant: Variant,
        python: PathBuf,
        tempocnn_model: Option<PathBuf>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(&python);
        cmd.arg("-c")
            .arg(BRIDGE)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(model) = &tempocnn_model {
            cmd.env("ESSENTIA_TEMPOCNN", model);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", python.display()))?;
        let stdin = child.stdin.take().ok_or("no worker stdin")?;
        let stdout = BufReader::new(child.stdout.take().ok_or("no worker stdout")?);
        let mut worker = Worker {
            child,
            stdin,
            stdout,
        };
        let hello = worker.read_json()?;
        let version = hello["version"]
            .as_str()
            .ok_or("worker did not report a version")?
            .to_string();
        Ok(Self {
            variant,
            version,
            worker,
        })
    }

    /// Start from `ESSENTIA_PYTHON` (and `ESSENTIA_TEMPOCNN`); `None` if not configured.
    pub fn from_env(variant: Variant) -> Option<Result<Self, String>> {
        let python = PathBuf::from(std::env::var_os("ESSENTIA_PYTHON")?);
        let model = std::env::var_os("ESSENTIA_TEMPOCNN").map(PathBuf::from);
        if variant == Variant::TempoCnn && model.is_none() {
            return None;
        }
        Some(Self::start(variant, python, model))
    }
}

impl Worker {
    fn read_json(&mut self) -> Result<Value, String> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| format!("worker: {e}"))?;
        if n == 0 {
            return Err("Essentia worker exited".into());
        }
        let v: Value =
            serde_json::from_str(&line).map_err(|e| format!("worker sent {line:?}: {e}"))?;
        if let Some(e) = v.get("error").and_then(Value::as_str) {
            return Err(e.to_string());
        }
        Ok(v)
    }

    fn request(&mut self, variant: &str, mono: &[f32]) -> Result<Value, String> {
        let header = serde_json::json!({ "variant": variant, "frames": mono.len() });
        let mut bytes = Vec::with_capacity(mono.len() * 4);
        for s in mono {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        let io = |e: std::io::Error| format!("worker: {e}");
        writeln!(self.stdin, "{header}").map_err(io)?;
        self.stdin.write_all(&bytes).map_err(io)?;
        self.stdin.flush().map_err(io)?;
        self.read_json()
    }
}

impl Analyzer for Essentia {
    fn id(&self) -> AnalyzerId {
        let (name, params) = match self.variant {
            Variant::Dsp => ("essentia", "rhythm=multifeature,key=edma"),
            Variant::TempoCnn => ("essentia-tempocnn", "deeptemp-k16-3"),
        };
        AnalyzerId {
            name: name.into(),
            version: self.version.clone(),
            params: params.into(),
        }
    }

    fn features(&self) -> FeatureSet {
        match self.variant {
            Variant::Dsp => FeatureSet::ALL,
            Variant::TempoCnn => Feature::Tempo.into(),
        }
    }

    fn analyze(
        &mut self,
        audio: &TrackAudio,
        want: FeatureSet,
        _context: &[Analysis],
        cancel: &CancelFlag,
    ) -> Result<Analysis, AnalysisError> {
        cancel.check()?;
        let audio = audio
            .at_rate(ANALYSIS_RATE)
            .map_err(AnalysisError::Backend)?;
        let variant = match self.variant {
            Variant::Dsp => "dsp",
            Variant::TempoCnn => "tempocnn",
        };
        let v = self
            .worker
            .request(variant, &audio.mono())
            .map_err(AnalysisError::Backend)?;
        cancel.check()?;

        let provenance = Provenance::Analyzer(self.id());
        let f32_at = |v: &Value| v.as_f64().map(|x| x as f32);
        let mut out = Analysis::default();
        if want.contains(Feature::Tempo)
            && let Some(bpm) = v["tempo"]["bpm"].as_f64().filter(|b| *b > 0.0)
        {
            out.tempo = Some(Analysed {
                value: Tempo { bpm },
                confidence: f32_at(&v["tempo"]["confidence"]),
                provenance: provenance.clone(),
            });
        }
        if want.contains(Feature::Beatgrid)
            && let Some(beats) = v["beats"].as_array().filter(|b| !b.is_empty())
        {
            out.beatgrid = Some(Analysed {
                value: Beatgrid::from_times(beats.iter().filter_map(Value::as_f64)),
                confidence: f32_at(&v["grid_confidence"]),
                provenance: provenance.clone(),
            });
        }
        if want.contains(Feature::Key)
            && let (Some(k), Some(scale)) = (v["key"]["key"].as_str(), v["key"]["scale"].as_str())
            && let Some(key) = Key::parse(&format!("{k} {scale}"))
        {
            out.key = Some(Analysed {
                value: key,
                confidence: f32_at(&v["key"]["confidence"]),
                provenance,
            });
        }
        Ok(out)
    }
}
