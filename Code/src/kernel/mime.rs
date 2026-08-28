//! Extension → MIME. Unknown → octet-stream. Criticality C1.

/// Domain: URL or filesystem path. Uses the last `.` in the last segment.
pub fn from_path(path: &str) -> &'static str {
    let seg = path.rsplit('/').next().unwrap_or(path);
    let ext = match seg.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.bytes().all(|b| b.is_ascii()) => e,
        _ => return "application/octet-stream",
    };
    let mut buf = [0u8; 16];
    if ext.len() > buf.len() {
        return "application/octet-stream";
    }
    for (i, b) in ext.bytes().enumerate() {
        buf[i] = b.to_ascii_lowercase();
    }
    match std::str::from_utf8(&buf[..ext.len()]).unwrap_or("") {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "xml" => "application/xml",
        "csv" => "text/csv; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "tar" => "application/x-tar",
        "bin" => "application/octet-stream",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_mime() {
        assert_eq!(from_path("/index.html"), "text/html; charset=utf-8");
    }

    #[test]
    fn unknown_ext_is_octet() {
        assert_eq!(from_path("/x.unknownext"), "application/octet-stream");
    }

    #[test]
    fn json_mime() {
        assert_eq!(from_path("/a.json"), "application/json");
    }

    #[test]
    fn table() {
        let pairs = [
            ("a.htm", "text/html; charset=utf-8"),
            ("a.css", "text/css; charset=utf-8"),
            ("a.js", "text/javascript; charset=utf-8"),
            ("a.mjs", "text/javascript; charset=utf-8"),
            ("a.txt", "text/plain; charset=utf-8"),
            ("a.md", "text/markdown; charset=utf-8"),
            ("a.xml", "application/xml"),
            ("a.csv", "text/csv; charset=utf-8"),
            ("a.png", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.svg", "image/svg+xml"),
            ("a.webp", "image/webp"),
            ("a.ico", "image/x-icon"),
            ("a.bmp", "image/bmp"),
            ("a.avif", "image/avif"),
            ("a.mp3", "audio/mpeg"),
            ("a.wav", "audio/wav"),
            ("a.ogg", "audio/ogg"),
            ("a.mp4", "video/mp4"),
            ("a.webm", "video/webm"),
            ("a.wasm", "application/wasm"),
            ("a.woff", "font/woff"),
            ("a.woff2", "font/woff2"),
            ("a.ttf", "font/ttf"),
            ("a.otf", "font/otf"),
            ("a.pdf", "application/pdf"),
            ("a.zip", "application/zip"),
            ("a.gz", "application/gzip"),
            ("a.tar", "application/x-tar"),
            ("a.bin", "application/octet-stream"),
            ("a.toml", "application/toml"),
            ("a.yaml", "application/yaml"),
            ("a.yml", "application/yaml"),
            ("a.map", "application/json"),
            ("noext", "application/octet-stream"),
            ("a.PNG", "image/png"),
            ("/dir/x.JSON", "application/json"),
        ];
        for (p, want) in pairs {
            assert_eq!(from_path(p), want, "{p}");
        }
    }
}
