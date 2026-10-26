//! Turning a non-text document into plain text for a knowledge base. Text and
//! code are read directly in [`crate::knowledge`]; this is where PDFs, Office
//! files and images (OCR) become text. Each extractor is best-effort: a failure
//! is reported to the agent, not fatal, and empty output is treated as a
//! failure so a scanned PDF does not look like an empty document.

use std::io::Read;
use std::path::Path;

use crate::knowledge::Kind;

/// Extracts the text of `path`, whose kind has already been decided.
pub fn extract(path: &Path, kind: Kind) -> Result<String, String> {
    let text = match kind {
        Kind::Pdf => pdf(path),
        Kind::Office => office(path),
        Kind::Image => ocr(path),
        Kind::Text | Kind::Other => return Err("godesk: this file is not extractable".into()),
    }?;
    if text.trim().is_empty() {
        return Err("godesk: no text could be extracted from this file".into());
    }
    Ok(text)
}

/// A PDF's embedded text. A scanned PDF has none; it comes back empty and the
/// caller reports that (page-image OCR would need a PDF renderer).
///
/// `pdf_extract` parses untrusted PDFs and is known to panic on malformed ones;
/// a panic here would tear down the bridge request thread and reset the caller's
/// connection (curl would report a network error). So the call is caught and
/// turned into an ordinary error the agent can read.
fn pdf(path: &Path) -> Result<String, String> {
    let path = path.to_owned();
    std::panic::catch_unwind(move || pdf_extract::extract_text(&path))
        .map_err(|_| "godesk: this PDF could not be parsed (it may be malformed or unusual)".to_owned())?
        .map_err(|e| format!("godesk: cannot read the PDF: {e}"))
}

/// Text from a Word, PowerPoint, Excel or OpenDocument file.
fn office(path: &Path) -> Result<String, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "xlsx" | "xls" | "ods" => spreadsheet(path),
        "docx" => zip_xml_text(path, &["word/document.xml"]),
        "pptx" => pptx(path),
        "odt" | "odp" => zip_xml_text(path, &["content.xml"]),
        "doc" | "ppt" => Err("godesk: legacy .doc/.ppt is not supported — save it as .docx/.pptx".into()),
        _ => Err("godesk: unsupported Office file".into()),
    }
}

/// Every sheet of a spreadsheet as tab-separated rows under a `# sheet` header.
fn spreadsheet(path: &Path) -> Result<String, String> {
    use calamine::{Data, Reader, open_workbook_auto};
    let mut workbook = open_workbook_auto(path).map_err(|e| format!("godesk: cannot open the spreadsheet: {e}"))?;
    let mut out = String::new();
    // Collect the names first so the immutable borrow ends before the mutable
    // `worksheet_range` call below.
    let names = workbook.sheet_names().to_owned();
    for name in names {
        let Ok(range) = workbook.worksheet_range(&name) else { continue };
        out.push_str(&format!("# {name}\n"));
        for row in range.rows() {
            let cells: Vec<String> = row
                .iter()
                .map(|cell| match cell {
                    Data::Empty => String::new(),
                    Data::String(s) => s.clone(),
                    Data::Float(f) => f.to_string(),
                    Data::Int(i) => i.to_string(),
                    Data::Bool(b) => b.to_string(),
                    other => other.to_string(),
                })
                .collect();
            out.push_str(&cells.join("\t"));
            out.push('\n');
        }
        out.push('\n');
    }
    Ok(out)
}

/// Joins the text of every slide of a .pptx, in slide order.
fn pptx(path: &Path) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("godesk: cannot open the file: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("godesk: cannot open the file: {e}"))?;
    let mut slides: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_owned()))
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .collect();
    // slide2 before slide10: compare by the number in the name.
    slides.sort_by_key(|name| slide_number(name));
    let mut out = String::new();
    for name in slides {
        if let Ok(mut entry) = zip.by_name(&name) {
            let mut xml = String::new();
            if entry.read_to_string(&mut xml).is_ok() {
                out.push_str(&xml_to_text(&xml));
                out.push('\n');
            }
        }
    }
    Ok(out)
}

/// The slide number in `ppt/slides/slideN.xml`, for ordering.
fn slide_number(name: &str) -> u32 {
    name.trim_start_matches("ppt/slides/slide").trim_end_matches(".xml").parse().unwrap_or(u32::MAX)
}

/// Reads the given XML parts of a zip-based document and renders their text.
fn zip_xml_text(path: &Path, entries: &[&str]) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("godesk: cannot open the file: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("godesk: cannot open the file: {e}"))?;
    let mut out = String::new();
    for name in entries {
        if let Ok(mut entry) = zip.by_name(name) {
            let mut xml = String::new();
            if entry.read_to_string(&mut xml).is_ok() {
                out.push_str(&xml_to_text(&xml));
            }
        }
    }
    Ok(out)
}

/// The visible text of an Office/OpenDocument XML part: the character data, with
/// a line break at each paragraph, line break or tab element. Works across the
/// Word (`w:`), PowerPoint/Drawing (`a:`) and OpenDocument (`text:`) namespaces,
/// since it keys off the local tag name only.
fn xml_to_text(xml: &str) -> String {
    use quick_xml::events::Event;
    use quick_xml::reader::Reader;
    let mut reader = Reader::from_str(xml);
    let mut out = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Text(text)) => {
                if let Ok(unescaped) = text.unescape() {
                    out.push_str(&unescaped);
                }
            }
            Ok(Event::End(end)) => {
                let name = end.name();
                if matches!(name.local_name().as_ref(), b"p" | b"br" | b"tab" | b"cr") {
                    out.push('\n');
                }
            }
            Ok(Event::Empty(empty)) => {
                let name = empty.name();
                if matches!(name.local_name().as_ref(), b"br" | b"tab" | b"cr") {
                    out.push('\n');
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    out
}

/// OCRs an image with Tesseract, if it is installed and on PATH. Without it, the
/// agent is told how to enable image reading rather than getting a hard error.
fn ocr(path: &Path) -> Result<String, String> {
    let mut command = std::process::Command::new("tesseract");
    command.arg(path).arg("stdout");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window of its own.
        command.creation_flags(0x0800_0000);
    }
    match command.output() {
        Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout).into_owned()),
        Ok(_) => Err("godesk: Tesseract could not read this image".into()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err("godesk: reading images needs Tesseract (OCR) installed and on PATH".into())
        }
        Err(err) => Err(format!("godesk: cannot run Tesseract: {err}")),
    }
}
