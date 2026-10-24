//! Offline speech-to-text with whisper.cpp. A prebuilt CPU binary and a model
//! are downloaded once into the app's data folder; recorded audio (a 16 kHz
//! mono WAV captured in the webview) is transcribed by running the binary.
//! Nothing leaves the machine and no API key is needed. Windows x64 for now:
//! that is the prebuilt binary we fetch.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::error::{AppError, AppResult};

/// A prebuilt CPU build of whisper.cpp for Windows x64 (a pinned CI build).
const BINARY_ZIP: &str = "https://github.com/ggml-org/whisper.cpp/releases/download/b5454/whisper-bin-x64.zip";
/// The multilingual tiny model (~75 MB): the fastest, for live dictation.
const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin";
const MODEL_NAME: &str = "ggml-tiny.bin";
/// Read buffer for streaming a download to disk.
const CHUNK: usize = 256 * 1024;

/// Keeps the spawned console program from flashing a window on Windows. Without
/// `CREATE_NO_WINDOW` the whisper binary pops up a black `cmd`-like window each
/// time we run it; on other platforms this is a no-op.
fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Where the binary and model live.
fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("whisper")
}

fn model_path(data_dir: &Path) -> PathBuf {
    dir(data_dir).join(MODEL_NAME)
}

/// The whisper CLI executable inside the extracted folder (its name changed
/// from `main` to `whisper-cli` across versions).
fn exe_path(data_dir: &Path) -> Option<PathBuf> {
    let d = dir(data_dir);
    ["whisper-cli.exe", "main.exe"].into_iter().map(|name| d.join(name)).find(|p| p.is_file())
}

/// The persistent server executable, which keeps the model loaded so repeated
/// requests are fast enough for live, word-by-word dictation.
fn server_exe(data_dir: &Path) -> Option<PathBuf> {
    let p = dir(data_dir).join("whisper-server.exe");
    p.is_file().then_some(p)
}

/// Whether the model and the one-shot binary are both present and ready to run.
pub fn ready(data_dir: &Path) -> bool {
    model_path(data_dir).is_file() && exe_path(data_dir).is_some()
}

/// Whether live (streaming) dictation is possible: the server keeps the model
/// loaded, so the webview can re-transcribe the growing audio every second.
pub fn can_stream(data_dir: &Path) -> bool {
    model_path(data_dir).is_file() && server_exe(data_dir).is_some()
}

/// An agent wired to the OS TLS stack (SChannel on Windows). With default
/// features off, ureq has no TLS backend until the connector is set here.
fn agent() -> AppResult<ureq::Agent> {
    let connector =
        native_tls::TlsConnector::new().map_err(|e| AppError::Invalid(format!("TLS init failed: {e}")))?;
    Ok(ureq::builder().tls_connector(std::sync::Arc::new(connector)).build())
}

/// Streams `url` to `to`, reporting `(received, total)` as it goes. Writes to a
/// `.part` file first so a cut-off download never looks complete.
fn download(url: &str, to: &Path, progress: &impl Fn(u64, u64)) -> AppResult<()> {
    let response = agent()?.get(url).call().map_err(|e| AppError::Invalid(format!("download failed: {e}")))?;
    let total: u64 = response.header("Content-Length").and_then(|l| l.parse().ok()).unwrap_or(0);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let part = to.with_extension("part");
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&part)?;
    let mut buf = vec![0u8; CHUNK];
    let mut received: u64 = 0;
    loop {
        let read = reader.read(&mut buf).map_err(|e| AppError::Invalid(format!("download read failed: {e}")))?;
        if read == 0 {
            break;
        }
        file.write_all(&buf[..read])?;
        received += read as u64;
        progress(received, total);
    }
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, to)?;
    Ok(())
}

/// Extracts every file of `zip_path` straight into `into` (flattened), so the
/// executable and its DLLs end up side by side whatever folders the zip used.
fn extract(zip_path: &Path, into: &Path) -> AppResult<()> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| AppError::Invalid(format!("bad zip: {e}")))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| AppError::Invalid(format!("bad zip entry: {e}")))?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry.enclosed_name().and_then(|p| p.file_name().map(|n| n.to_owned())) else {
            continue;
        };
        let mut dest = std::fs::File::create(into.join(name))?;
        std::io::copy(&mut entry, &mut dest)?;
    }
    Ok(())
}

/// Downloads the model and the binary if they are missing, reporting progress
/// as JSON (`phase`: "model" | "binary" | "done", with `received`/`total`).
pub fn install(data_dir: &Path, emit: &impl Fn(serde_json::Value)) -> AppResult<()> {
    let d = dir(data_dir);
    std::fs::create_dir_all(&d)?;
    if !model_path(data_dir).is_file() {
        download(MODEL_URL, &model_path(data_dir), &|received, total| {
            emit(json!({ "phase": "model", "received": received, "total": total }));
        })?;
    }
    if exe_path(data_dir).is_none() {
        let zip = d.join("whisper-bin-x64.zip");
        download(BINARY_ZIP, &zip, &|received, total| {
            emit(json!({ "phase": "binary", "received": received, "total": total }));
        })?;
        extract(&zip, &d)?;
        let _ = std::fs::remove_file(&zip);
        if exe_path(data_dir).is_none() {
            return Err(AppError::Invalid("the voice engine download did not contain whisper-cli".into()));
        }
    }
    emit(json!({ "phase": "done" }));
    Ok(())
}

/// The language to transcribe in: a two-letter code like "es", or "auto".
fn language(lang: &str) -> String {
    let code = lang.split(['-', '_']).next().unwrap_or("").to_lowercase();
    if code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic()) { code } else { "auto".to_owned() }
}

/// Transcribes WAV bytes in `lang` (e.g. "es", or "auto"). Uses the persistent
/// server when it is installed (fast enough for live updates); otherwise falls
/// back to the one-shot binary, which reloads the model each time.
pub fn transcribe(data_dir: &Path, wav: &[u8], lang: &str) -> AppResult<String> {
    let lang = language(lang);
    if server_exe(data_dir).is_some() {
        return transcribe_via_server(data_dir, wav, &lang);
    }
    transcribe_via_cli(data_dir, wav, &lang)
}

/// Runs the one-shot binary on a temp WAV written from `wav`. Slow (the model
/// is loaded every call), so it is the fallback when the server is absent.
fn transcribe_via_cli(data_dir: &Path, wav: &[u8], lang: &str) -> AppResult<String> {
    let exe = exe_path(data_dir).ok_or_else(|| AppError::Invalid("the voice model is not installed yet".into()))?;
    let d = dir(data_dir);
    std::fs::create_dir_all(&d)?;
    let wav_path = d.join(format!("rec-{}.wav", uuid::Uuid::new_v4().simple()));
    std::fs::write(&wav_path, wav)?;
    let base = wav_path.with_extension("");
    let txt = wav_path.with_extension("txt");
    let result = (|| {
        let mut command = Command::new(&exe);
        let output = hidden(&mut command)
            .arg("-m")
            .arg(model_path(data_dir))
            .arg("-f")
            .arg(&wav_path)
            .args(["-l", lang, "-nt", "-otxt"])
            .arg("-of")
            .arg(&base)
            .current_dir(&d)
            .output()
            .map_err(|e| AppError::Invalid(format!("cannot run the voice engine: {e}")))?;
        if !output.status.success() {
            let why = String::from_utf8_lossy(&output.stderr);
            let why = why.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("unknown error");
            return Err(AppError::Invalid(format!("transcription failed: {}", why.trim())));
        }
        Ok(std::fs::read_to_string(&txt).unwrap_or_default())
    })();
    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&txt);
    Ok(result?.split_whitespace().collect::<Vec<_>>().join(" "))
}

// ---------- Persistent server, for live dictation ----------

struct Server {
    child: Child,
    port: u16,
}

/// The one running whisper server (loaded model), shared across requests.
static SERVER: LazyLock<Mutex<Option<Server>>> = LazyLock::new(|| Mutex::new(None));

/// A free localhost port to hand the server.
fn free_port() -> AppResult<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// Starts the server if it is not already running, and returns its port. The
/// model is loaded once, here, not on every request.
fn ensure_server(data_dir: &Path) -> AppResult<u16> {
    let mut guard = SERVER.lock().expect("whisper server lock poisoned");
    if let Some(server) = guard.as_mut() {
        // Still alive? `try_wait` is Ok(None) while it runs.
        if matches!(server.child.try_wait(), Ok(None)) {
            return Ok(server.port);
        }
        *guard = None;
    }
    let exe = server_exe(data_dir).ok_or_else(|| AppError::Invalid("the voice server is not installed".into()))?;
    let port = free_port()?;
    // Use most of the machine's cores (whisper defaults to only 4), and skip the
    // temperature fallback retries, so live transcription keeps up with speech.
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().clamp(4, 12));
    let mut command = Command::new(&exe);
    hidden(&mut command)
        .arg("-m")
        .arg(model_path(data_dir))
        .args(["--host", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .arg("-t")
        .arg(threads.to_string())
        .arg("-nf")
        .current_dir(dir(data_dir))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|e| AppError::Invalid(format!("cannot start the voice server: {e}")))?;
    // Wait for it to accept connections (loading the model takes a moment).
    let base = format!("http://127.0.0.1:{port}/");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if ureq::get(&base).timeout(Duration::from_millis(500)).call().is_ok() {
            *guard = Some(Server { child, port });
            return Ok(port);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    // Never came up: stop it so no orphan lingers.
    let mut child = child;
    let _ = child.kill();
    Err(AppError::Invalid("the voice server did not start in time".into()))
}

/// A multipart `name=value` text field.
fn field(boundary: &str, name: &str, value: &str) -> String {
    format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n")
}

/// Posts the WAV to the running server's `/inference` endpoint and returns the
/// transcript. Multipart is built by hand (ureq has no multipart helper).
fn transcribe_via_server(data_dir: &Path, wav: &[u8], lang: &str) -> AppResult<String> {
    let port = ensure_server(data_dir)?;
    let boundary = format!("godesk{}", uuid::Uuid::new_v4().simple());
    let mut body = Vec::new();
    let head = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
    );
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(wav);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(field(&boundary, "response_format", "text").as_bytes());
    body.extend_from_slice(field(&boundary, "language", lang).as_bytes());
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let response = ureq::post(&format!("http://127.0.0.1:{port}/inference"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .timeout(Duration::from_secs(60))
        .send_bytes(&body)
        .map_err(|e| AppError::Invalid(format!("transcription failed: {e}")))?;
    let text = response.into_string().map_err(|e| AppError::Invalid(format!("transcription read failed: {e}")))?;
    Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Stops the server, if one is running (called when the app exits).
pub fn shutdown() {
    if let Ok(mut guard) = SERVER.lock() {
        if let Some(mut server) = guard.take() {
            let _ = server.child.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "hits the network to confirm the TLS backend actually works"]
    fn tls_agent_reaches_the_model_host() {
        // `.call()` reads only the headers, not the ~148 MB body: enough to
        // prove the HTTPS handshake succeeded through SChannel.
        let response = agent().expect("agent").get(MODEL_URL).call();
        match response {
            Ok(r) => assert_eq!(r.status(), 200, "unexpected status"),
            Err(ureq::Error::Status(code, _)) => panic!("server said {code}, but TLS worked"),
            Err(e) => panic!("TLS/transport failed: {e}"),
        }
    }

    #[test]
    fn not_ready_in_an_empty_folder() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!ready(tmp.path()));
        assert!(exe_path(tmp.path()).is_none());
    }

    #[test]
    fn extract_flattens_into_one_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("bin.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut w = zip::ZipWriter::new(file);
            let opts: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            w.start_file("Release/whisper-cli.exe", opts).unwrap();
            w.write_all(b"exe").unwrap();
            w.start_file("Release/whisper.dll", opts).unwrap();
            w.write_all(b"dll").unwrap();
            w.finish().unwrap();
        }
        let into = tmp.path().join("out");
        std::fs::create_dir_all(&into).unwrap();
        extract(&zip_path, &into).unwrap();
        assert!(into.join("whisper-cli.exe").is_file());
        assert!(into.join("whisper.dll").is_file());
    }
}
