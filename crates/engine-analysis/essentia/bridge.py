"""Essentia worker for engine-analysis (run by the Rust `Essentia` analyzer).

Protocol on stdin/stdout, one request at a time:
  request:  one JSON line {"variant": "dsp" | "tempocnn", "frames": N}, then N little-endian
            float32 mono samples at 44.1 kHz (raw bytes)
  response: one JSON line with the results, or {"error": "..."}

Essentia is AGPL-3.0; the TempoCNN model (deeptemp-k16-3.pb) is AGPL-3.0 too. Neither is
bundled: the model path comes from the ESSENTIA_TEMPOCNN environment variable.
"""

import json
import os
import sys
import time

os.environ.setdefault("TF_CPP_MIN_LOG_LEVEL", "3")

import numpy as np  # noqa: E402
import essentia  # noqa: E402
import essentia.standard as es  # noqa: E402

essentia.log.infoActive = False
essentia.log.warningActive = False

SR = 44100
_tempocnn = None


def dsp(audio):
    bpm, beats, confidence, _, _ = es.RhythmExtractor2013(method="multifeature")(audio)
    key, scale, strength = es.KeyExtractor(profileType="edma", sampleRate=SR)(audio)
    return {
        # multifeature confidence runs 0..5.32 ("above 3.5 is excellent").
        "tempo": {"bpm": float(bpm), "confidence": float(min(confidence / 5.32, 1.0))},
        "beats": [float(b) for b in beats],
        "grid_confidence": float(min(confidence / 5.32, 1.0)),
        "key": {"key": key, "scale": scale, "confidence": float(strength)},
    }


def tempocnn(audio):
    global _tempocnn
    if _tempocnn is None:
        model = os.environ.get("ESSENTIA_TEMPOCNN")
        if not model or not os.path.exists(model):
            raise RuntimeError("ESSENTIA_TEMPOCNN must point to deeptemp-k16-3.pb")
        _tempocnn = es.TempoCNN(graphFilename=model)
    low = es.Resample(inputSampleRate=SR, outputSampleRate=11025)(audio)
    global_bpm, local_bpm, local_prob = _tempocnn(low)
    agree = float(np.mean(np.asarray(local_bpm) == global_bpm)) if len(local_bpm) else 0.0
    return {"tempo": {"bpm": float(global_bpm), "confidence": agree}}


def main():
    stdin = sys.stdin.buffer
    stdout = sys.stdout
    stdout.write(json.dumps({"ready": True, "version": essentia.__version__}) + "\n")
    stdout.flush()
    while True:
        line = stdin.readline()
        if not line:
            return
        req = json.loads(line)
        raw = stdin.read(4 * int(req["frames"]))
        try:
            audio = np.frombuffer(raw, dtype="<f4").astype(np.float32)
            started = time.perf_counter()
            if req["variant"] == "dsp":
                out = dsp(audio)
            elif req["variant"] == "tempocnn":
                out = tempocnn(audio)
            else:
                raise ValueError(f"unknown variant {req['variant']!r}")
            out["seconds"] = time.perf_counter() - started
        except Exception as e:  # report, keep serving
            out = {"error": f"{type(e).__name__}: {e}"}
        stdout.write(json.dumps(out) + "\n")
        stdout.flush()


if __name__ == "__main__":
    main()
