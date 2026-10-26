// Records the microphone to a 16 kHz mono 16-bit WAV — the format whisper.cpp
// expects — entirely in the webview, so the audio can be transcribed offline.
// Capture is through the Web Audio API (not MediaRecorder), so the samples are
// raw PCM that needs no decoding on the Rust side.

export type Recorder = {
  /** The WAV of everything captured so far, without stopping (for live updates). */
  snapshot: () => Uint8Array;
  /** Ends recording and returns the final WAV. */
  stop: () => Promise<Uint8Array>;
};

const TARGET_RATE = 16000;

function concat(chunks: Float32Array[]): Float32Array {
  let length = 0;
  for (const chunk of chunks) length += chunk.length;
  const out = new Float32Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.length;
  }
  return out;
}

/** Linear resample of mono samples from `from` Hz to `to` Hz. */
function resample(input: Float32Array, from: number, to: number): Float32Array {
  if (from === to || input.length === 0) return input;
  const ratio = from / to;
  const length = Math.max(1, Math.floor(input.length / ratio));
  const out = new Float32Array(length);
  for (let i = 0; i < length; i++) {
    const pos = i * ratio;
    const i0 = Math.floor(pos);
    const i1 = Math.min(i0 + 1, input.length - 1);
    const frac = pos - i0;
    out[i] = (input[i0] ?? 0) * (1 - frac) + (input[i1] ?? 0) * frac;
  }
  return out;
}

/** A 16-bit PCM mono WAV from float samples. */
function encodeWav(samples: Float32Array, rate: number): Uint8Array {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);
  const text = (offset: number, value: string) => {
    for (let i = 0; i < value.length; i++) view.setUint8(offset + i, value.charCodeAt(i));
  };
  text(0, "RIFF");
  view.setUint32(4, 36 + samples.length * 2, true);
  text(8, "WAVE");
  text(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, rate, true);
  view.setUint32(28, rate * 2, true); // byte rate
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  text(36, "data");
  view.setUint32(40, samples.length * 2, true);
  let offset = 44;
  for (let i = 0; i < samples.length; i++) {
    const s = Math.max(-1, Math.min(1, samples[i] ?? 0));
    view.setInt16(offset, s < 0 ? s * 0x8000 : s * 0x7fff, true);
    offset += 2;
  }
  return new Uint8Array(buffer);
}

/** The microphones the browser can see. Labels need prior mic permission;
 * until then they fall back to "Microphone N". */
export async function listMicrophones(): Promise<{ id: string; label: string }[]> {
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    return devices
      .filter((d) => d.kind === "audioinput")
      .map((d, i) => ({ id: d.deviceId, label: d.label || `Microphone ${i + 1}` }));
  } catch {
    return [];
  }
}

type AudioContextCtor = typeof AudioContext;

function audioContextCtor(): AudioContextCtor | null {
  const w = window as unknown as { AudioContext?: AudioContextCtor; webkitAudioContext?: AudioContextCtor };
  return w.AudioContext ?? w.webkitAudioContext ?? null;
}

/**
 * Starts recording. Resolves once the microphone is open (which may prompt for
 * permission); the returned `stop` ends it and resolves to the WAV bytes.
 */
export async function startRecording(deviceId?: string): Promise<Recorder> {
  const Ctx = audioContextCtor();
  if (!Ctx) throw new Error("Audio capture is not available in this window.");
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
    },
  });
  const ctx = new Ctx();
  const source = ctx.createMediaStreamSource(stream);
  const processor = ctx.createScriptProcessor(4096, 1, 1);
  // A muted sink keeps the processor pulling audio without playing it back.
  const sink = ctx.createGain();
  sink.gain.value = 0;
  const chunks: Float32Array[] = [];
  processor.onaudioprocess = (event) => {
    chunks.push(new Float32Array(event.inputBuffer.getChannelData(0)));
  };
  source.connect(processor);
  processor.connect(sink);
  sink.connect(ctx.destination);
  const inputRate = ctx.sampleRate;

  return {
    snapshot: () => encodeWav(resample(concat(chunks), inputRate, TARGET_RATE), TARGET_RATE),
    stop: async () => {
      processor.onaudioprocess = null;
      processor.disconnect();
      source.disconnect();
      sink.disconnect();
      for (const track of stream.getTracks()) track.stop();
      await ctx.close().catch(() => undefined);
      return encodeWav(resample(concat(chunks), inputRate, TARGET_RATE), TARGET_RATE);
    },
  };
}
