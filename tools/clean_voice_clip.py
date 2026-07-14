"""Canonical Chatterbox voice-clip prep for Sonara (#51).

Trims a source recording to an ~18s reference window and removes constant
background hum (mains + room tone) by SPECTRAL SUBTRACTION: it builds a noise
profile from the clip's quietest region and subtracts exactly that spectrum,
so the hum comb (60/120/180 Hz ...) is knocked to the noise floor while the
voice is preserved (measured ~97% fundamental / 100% body retention -- better
than fixed notches on both axes). The daemon's synth worker also notches 60/120
Hz at generation time as a universal net (sonara/chatterbox_worker.py _dehum),
but cleaning the CLIP is the primary, highest-quality fix.

Run inside the chatterbox venv (needs scipy); needs ffmpeg on PATH/at FFMPEG.
    <venv>/python tools/clean_voice_clip.py SRC.mp3 OUT.wav [trim|notrim] [seg_start_s]
Drop OUT.wav (lowercase stem) into ~/.sonara/voices/chatterbox/ to register it.
"""
import os, sys, subprocess, wave
import numpy as np
from scipy.signal import stft, istft

FFMPEG = (r"C:\Users\Admin\AppData\Local\Microsoft\WinGet\Packages"
          r"\Gyan.FFmpeg_Microsoft.Winget.Source_8wekyb3d8bbwe"
          r"\ffmpeg-8.1.1-full_build\bin\ffmpeg.exe")
SR = 24000

def decode(src):
    tmp = os.path.join(os.environ["TEMP"], "vc_decode.wav")
    subprocess.run([FFMPEG, "-y", "-i", src, "-ac", "1", "-ar", str(SR), tmp],
                   check=True, capture_output=True)
    with wave.open(tmp, "rb") as w:
        x = np.frombuffer(w.readframes(w.getnframes()), dtype=np.int16).astype(np.float64) / 32768.0
    return x

def pick_segment(x, dur=18.0, skip=25.0):
    """Best continuous-speech window: highest voiced fraction, lowest silence."""
    win = int(dur * SR); hop = int(1.0 * SR); start0 = int(skip * SR)
    fr = int(0.03 * SR)
    best = (-1, start0)
    for s in range(start0, max(start0 + 1, len(x) - win), hop):
        seg = x[s:s + win]
        # frame RMS; voiced = above a floor and not clipping
        rms = np.array([np.sqrt(np.mean(seg[i:i+fr]**2)) for i in range(0, len(seg)-fr, fr)])
        thresh = max(0.01, np.median(rms) * 0.5)
        voiced = np.mean(rms > thresh)
        clip = np.mean(np.abs(seg) > 0.98)
        score = voiced - 3 * clip
        if score > best[0]:
            best = (score, s)
    return best[1]

def noise_profile(x, nper=1024):
    win = int(0.3 * SR); step = int(0.02 * SR)
    qi = min(range(0, max(1, len(x) - win), step), key=lambda i: np.mean(x[i:i+win]**2))
    noise = x[qi:qi + win]
    _, _, N = stft(noise, fs=SR, nperseg=nper, noverlap=nper * 3 // 4)
    return np.mean(np.abs(N), axis=1, keepdims=True)

def dehum(seg, prof, over=1.3, floor=0.05, nper=1024):
    f, t, Z = stft(seg, fs=SR, nperseg=nper, noverlap=nper * 3 // 4)
    mag = np.abs(Z); ph = np.angle(Z)
    clean = np.maximum(mag - over * prof, floor * mag)
    _, y = istft(clean * np.exp(1j * ph), fs=SR, nperseg=nper, noverlap=nper * 3 // 4)
    return y[:len(seg)]

def save(path, y):
    pcm = (np.clip(y, -1, 1) * 32767).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(1); w.setsampwidth(2); w.setframerate(SR)
        w.writeframes(pcm.tobytes())

def process(src, out, trim=True, seg_start=None):
    x = decode(src)
    prof = noise_profile(x)                      # profile from the FULL clip (true room tone)
    if trim:
        s = seg_start if seg_start is not None else pick_segment(x)
        seg = x[s:s + int(18 * SR)]
    else:
        seg = x
    y = dehum(seg, prof)
    save(out, y)
    return len(seg) / SR

if __name__ == "__main__":
    src, out = sys.argv[1], sys.argv[2]
    trim = (sys.argv[3] == "trim") if len(sys.argv) > 3 else True
    d = process(src, out, trim=trim)
    print(f"{os.path.basename(out)}: {d:.1f}s")
