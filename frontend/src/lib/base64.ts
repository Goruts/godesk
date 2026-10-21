// PTY output and terminal input cross IPC as base64 so that UTF-8 sequences
// split across reads are never mangled by a string conversion.

const CHUNK = 0x8000;

export function encodeBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(i, i + CHUNK));
  }
  return btoa(binary);
}

export function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

const encoder = new TextEncoder();

export function encodeText(text: string): string {
  return encodeBase64(encoder.encode(text));
}

/** xterm's `onBinary` delivers one byte per UTF-16 code unit. */
export function encodeBinaryString(binary: string): string {
  return btoa(binary);
}
