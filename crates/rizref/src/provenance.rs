//! Where an image came from: source-URL capture and the provenance merge.
//!
//! Browsers describe a dragged or copied image in several clipboard formats
//! besides the pixels themselves: a `CF_HTML` fragment with an `<img src>`
//! and the page's `SourceURL`, a `UniformResourceLocatorW`, a `text/uri-list`
//! or Firefox's `text/x-moz-url`. This module turns those payloads into one
//! validated `http`/`https` URL, and decides what an asset's recorded
//! provenance becomes when an import lands on it.
//!
//! Every function here is pure. The payloads are untrusted — any application
//! can put anything on the clipboard — so parsing uses checked offsets and
//! size caps, never panics, and never fetches anything it finds. The OS
//! plumbing that reads the raw bytes lives in `clipboard` and
//! `platform::drop_target`.

// The drop and `CF_HTML` parsers serve the Windows OLE and clipboard paths
// today; the Linux and macOS drag payloads (`text/uri-list`, `text/x-moz-url`,
// `public.url`) are the phase-08 input work that reuses them. Their tests run
// on every OS.
#![cfg_attr(not(windows), allow(dead_code))]

use rizref_format::{AssetOrigin, AssetProvenance};

/// Longest URL accepted, in bytes. Longer values are refused rather than
/// truncated: a cut URL names a different resource.
pub const MAX_URL_BYTES: usize = 4096;

/// Largest `CF_HTML` / `text/html` payload parsed, bytes.
pub const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;

/// Largest URL-format payload (`UniformResourceLocator*`, uri-list,
/// moz-url) read, bytes.
pub const MAX_URL_PAYLOAD_BYTES: usize = 64 * 1024;

// ---------------------------------------------------------------------------
// URL validation and resolution
// ---------------------------------------------------------------------------

/// Validates a candidate URL and returns its canonical form.
///
/// Trims surrounding whitespace and quotes, requires an `http://` or
/// `https://` scheme (case-insensitive, lowercased on output) with a
/// non-empty host, and refuses control characters, embedded whitespace,
/// backslashes and anything longer than [`MAX_URL_BYTES`]. Percent-encoding
/// is preserved exactly; nothing is decoded.
pub fn normalize_url(candidate: &str) -> Option<String> {
    let trimmed = candidate
        .trim()
        .trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    if trimmed.is_empty() || trimmed.len() > MAX_URL_BYTES {
        return None;
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
    {
        return None;
    }
    let (scheme, rest) = trimmed.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if host_port.starts_with('[') {
        // An IPv6 literal keeps its colons; the port follows the bracket.
        host_port.split_once(']').map(|(host, _)| &host[1..])?
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{rest}"))
}

/// Whether `url` is a launchable `http`/`https` URL.
pub fn is_launchable(url: &str) -> bool {
    normalize_url(url).is_some()
}

/// Resolves `reference` against `base` and validates the result.
///
/// Handles absolute, scheme-relative (`//host/x`), root-relative (`/x`),
/// query-only, fragment-only and dot-segment paths. A relative reference
/// without a usable `http`/`https` base yields `None`.
pub fn resolve_url(base: Option<&str>, reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    if has_scheme(reference) {
        return normalize_url(reference);
    }
    let base = normalize_url(base?)?;
    let (scheme, rest) = base.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let after_authority = &rest[authority_end..];
    let base_path_end = after_authority
        .find(['?', '#'])
        .unwrap_or(after_authority.len());
    let base_path = &after_authority[..base_path_end];
    let base_query_end = after_authority.find('#').unwrap_or(after_authority.len());
    let base_without_fragment = &after_authority[..base_query_end];

    let joined = if let Some(scheme_relative) = reference.strip_prefix("//") {
        format!("{scheme}://{scheme_relative}")
    } else if reference.starts_with('#') {
        format!("{scheme}://{authority}{base_without_fragment}{reference}")
    } else if reference.starts_with('?') {
        let path = if base_path.is_empty() { "/" } else { base_path };
        format!("{scheme}://{authority}{path}{reference}")
    } else {
        let (path, suffix) = split_suffix(reference);
        let merged = if path.starts_with('/') {
            path.to_owned()
        } else {
            let directory = base_path.rfind('/').map_or("/", |at| &base_path[..=at]);
            format!("{directory}{path}")
        };
        format!(
            "{scheme}://{authority}{}{suffix}",
            remove_dot_segments(&merged)
        )
    };
    normalize_url(&joined)
}

/// Whether `text` starts with an RFC 3986 scheme followed by `:`.
fn has_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Splits a reference into its path and its `?query#fragment` suffix.
fn split_suffix(reference: &str) -> (&str, &str) {
    let at = reference.find(['?', '#']).unwrap_or(reference.len());
    reference.split_at(at)
}

/// RFC 3986 §5.2.4 dot-segment removal on an absolute path.
fn remove_dot_segments(path: &str) -> String {
    let mut output: Vec<&str> = Vec::new();
    let segments: Vec<&str> = path.split('/').collect();
    let last = segments.len().saturating_sub(1);
    for (index, segment) in segments.iter().enumerate() {
        match *segment {
            "." => {
                if index == last {
                    output.push("");
                }
            }
            ".." => {
                if output.len() > 1 {
                    output.pop();
                }
                if index == last {
                    output.push("");
                }
            }
            other => output.push(other),
        }
    }
    let joined = output.join("/");
    if joined.starts_with('/') {
        joined
    } else {
        format!("/{joined}")
    }
}

// ---------------------------------------------------------------------------
// CF_HTML
// ---------------------------------------------------------------------------

/// A parsed `CF_HTML` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfHtml {
    /// The `SourceURL` header, as written (unvalidated).
    pub source_url: Option<String>,
    /// The fragment between `StartFragment` and `EndFragment`.
    pub fragment: String,
    /// The whole HTML document between `StartHTML` and `EndHTML`, used to
    /// find a `<base href>` outside the fragment.
    pub document: String,
}

/// Parses a `CF_HTML` clipboard payload.
///
/// A payload that does not start with a `Version:` header is treated as a
/// bare HTML fragment (what `text/html` drags carry). A payload with a
/// header whose offsets are missing, out of range or reversed is malformed
/// and yields `None`, as does anything over [`MAX_HTML_BYTES`].
pub fn parse_cf_html(bytes: &[u8]) -> Option<CfHtml> {
    let bytes = trim_trailing_nuls(bytes);
    if bytes.len() > MAX_HTML_BYTES {
        return None;
    }
    if !bytes.starts_with(b"Version:") {
        let text = String::from_utf8_lossy(bytes).into_owned();
        return Some(CfHtml {
            source_url: None,
            fragment: text.clone(),
            document: text,
        });
    }

    let mut start_html = None;
    let mut end_html = None;
    let mut start_fragment = None;
    let mut end_fragment = None;
    let mut source_url = None;
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        if bytes[cursor] == b'<' {
            break;
        }
        let line_end = bytes[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |at| cursor + at);
        let line = String::from_utf8_lossy(&bytes[cursor..line_end]);
        let line = line.trim_end_matches('\r');
        cursor = line_end + 1;
        let Some((key, value)) = line.split_once(':') else {
            break;
        };
        let value = value.trim();
        match key {
            "StartHTML" => start_html = parse_offset(value),
            "EndHTML" => end_html = parse_offset(value),
            "StartFragment" => start_fragment = Some(parse_offset(value)?),
            "EndFragment" => end_fragment = Some(parse_offset(value)?),
            "SourceURL" => source_url = Some(value.to_owned()),
            _ => {}
        }
    }

    let (start_fragment, end_fragment) = (start_fragment?, end_fragment?);
    if start_fragment > end_fragment || end_fragment > bytes.len() {
        return None;
    }
    let fragment = String::from_utf8_lossy(&bytes[start_fragment..end_fragment]).into_owned();
    // StartHTML/EndHTML are optional (-1 is allowed); fall back to the
    // fragment when they are absent or inconsistent.
    let document = match (start_html, end_html) {
        (Some(start), Some(end)) if start <= end && end <= bytes.len() => {
            String::from_utf8_lossy(&bytes[start..end]).into_owned()
        }
        _ => fragment.clone(),
    };
    Some(CfHtml {
        source_url: source_url.filter(|url| !url.is_empty()),
        fragment,
        document,
    })
}

/// A non-negative decimal offset; `-1` and garbage are `None`.
fn parse_offset(value: &str) -> Option<usize> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

/// Drops the NUL padding some writers leave after the payload.
fn trim_trailing_nuls(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |at| at + 1);
    &bytes[..end]
}

/// The image URL a `CF_HTML` payload names, if any.
///
/// Prefers the first `<img src>` in the fragment, resolved against a
/// `<base href>` or the `SourceURL`; falls back to the `SourceURL` itself.
/// Only `http`/`https` results are returned.
pub fn url_from_cf_html(bytes: &[u8]) -> Option<String> {
    let html = parse_cf_html(bytes)?;
    let base = first_tag_attribute(&html.document, "base", "href")
        .and_then(|href| resolve_url(html.source_url.as_deref(), &href))
        .or_else(|| html.source_url.as_deref().and_then(normalize_url));
    first_tag_attribute(&html.fragment, "img", "src")
        .and_then(|src| resolve_url(base.as_deref(), &src))
        .or_else(|| html.source_url.as_deref().and_then(normalize_url))
}

/// The image URL plain clipboard HTML names (`text/html` on Linux,
/// `public.html` on macOS), if any.
///
/// Unlike `CF_HTML` there is no `SourceURL` header to resolve a relative
/// `src` against, so only an absolute `http`/`https` `<img src>` counts.
#[cfg_attr(windows, allow(dead_code))]
pub fn url_from_plain_html(html: &str) -> Option<String> {
    let base = first_tag_attribute(html, "base", "href").and_then(|href| normalize_url(&href));
    first_tag_attribute(html, "img", "src").and_then(|src| resolve_url(base.as_deref(), &src))
}

/// The `attribute` of the first `<tag …>` in `html`, entity-decoded.
///
/// A tolerant scan, not an HTML parser: it handles any case, single,
/// double or missing quotes and arbitrary whitespace, and gives up on the
/// tag (moving on to the next) when the markup is broken.
pub fn first_tag_attribute(html: &str, tag: &str, attribute: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let needle = format!("<{tag}");
    let mut from = 0;
    while let Some(found) = lower[from..].find(&needle) {
        let start = from + found + needle.len();
        from = start;
        let next = lower.as_bytes().get(start).copied();
        if !matches!(next, Some(b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>')) {
            continue;
        }
        if let Some(value) = scan_attributes(&html[start..], attribute) {
            return Some(decode_entities(&value));
        }
    }
    None
}

/// Scans one tag's attribute list for `wanted`, stopping at the tag's `>`.
fn scan_attributes(tag: &str, wanted: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut at = 0;
    loop {
        while at < bytes.len() && (bytes[at].is_ascii_whitespace() || bytes[at] == b'/') {
            at += 1;
        }
        if at >= bytes.len() || bytes[at] == b'>' {
            return None;
        }
        let name_start = at;
        while at < bytes.len()
            && !bytes[at].is_ascii_whitespace()
            && !matches!(bytes[at], b'=' | b'>' | b'/')
        {
            at += 1;
        }
        let name = &tag[name_start..at];
        while at < bytes.len() && bytes[at].is_ascii_whitespace() {
            at += 1;
        }
        let mut value = None;
        if at < bytes.len() && bytes[at] == b'=' {
            at += 1;
            while at < bytes.len() && bytes[at].is_ascii_whitespace() {
                at += 1;
            }
            match bytes.get(at) {
                Some(quote @ (b'"' | b'\'')) => {
                    let close = tag[at + 1..].find(*quote as char)?;
                    value = Some(&tag[at + 1..at + 1 + close]);
                    at += close + 2;
                }
                Some(_) => {
                    let value_start = at;
                    while at < bytes.len() && !bytes[at].is_ascii_whitespace() && bytes[at] != b'>'
                    {
                        at += 1;
                    }
                    value = Some(&tag[value_start..at]);
                }
                None => return None,
            }
        }
        if name.eq_ignore_ascii_case(wanted) {
            return value.map(str::to_owned);
        }
    }
}

/// Decodes the HTML character references an attribute value can carry.
///
/// Named `&amp; &lt; &gt; &quot; &apos;` and numeric `&#…;` / `&#x…;`
/// references; anything else is kept literally.
pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[1..]
            .find(';')
            .map(|end| end + 1)
            .filter(|end| *end <= 12)
        else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity.strip_prefix('#').and_then(|number| {
                let code = match number.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => number.parse().ok(),
                }?;
                char::from_u32(code)
            }),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// URL-only formats
// ---------------------------------------------------------------------------

/// A NUL-terminated little-endian UTF-16 string (`UniformResourceLocatorW`,
/// `text/x-moz-url`).
pub fn utf16_until_nul(bytes: &[u8]) -> String {
    let bytes = &bytes[..bytes.len().min(MAX_URL_PAYLOAD_BYTES)];
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// A NUL-terminated 8-bit string (`UniformResourceLocator`, uri-list).
pub fn bytes_until_nul(bytes: &[u8]) -> String {
    let bytes = &bytes[..bytes.len().min(MAX_URL_PAYLOAD_BYTES)];
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// A text payload of unknown width: browsers write `text/x-moz-url` as
/// UTF-16 and `text/uri-list` as either. A zero second byte means UTF-16
/// (every URL starts with an ASCII scheme letter).
pub fn text_payload(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[1] == 0 && bytes[0] != 0 {
        utf16_until_nul(bytes)
    } else {
        bytes_until_nul(bytes)
    }
}

/// The first URL of a `text/uri-list` (RFC 2483): lines starting with `#`
/// are comments.
pub fn first_uri(list: &str) -> Option<String> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .find_map(normalize_url)
}

/// The URL of a `text/x-moz-url` payload: the first line; the second is the
/// title.
pub fn moz_url(text: &str) -> Option<String> {
    text.lines().next().and_then(normalize_url)
}

/// The URL-bearing payloads one drop offered, raw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DropUrlPayloads {
    /// `UniformResourceLocatorW`, decoded.
    pub locator_wide: Option<String>,
    /// `UniformResourceLocator` (ANSI), decoded.
    pub locator_ansi: Option<String>,
    /// `text/uri-list`, decoded.
    pub uri_list: Option<String>,
    /// `text/x-moz-url`, decoded.
    pub moz_url: Option<String>,
    /// `HTML Format` (or `text/html`), raw bytes.
    pub html: Option<Vec<u8>>,
}

/// Picks the source URL for a drop.
///
/// The image's own `<img src>` wins, because a URL-locator format often
/// names the *link* an image sits inside. Then the URL formats in the
/// plan's order: `UniformResourceLocatorW/A`, `text/uri-list`,
/// `text/x-moz-url`, and finally the HTML's page `SourceURL`.
pub fn choose_drop_url(payloads: &DropUrlPayloads) -> Option<String> {
    let html = payloads.html.as_deref().and_then(parse_cf_html);
    if let Some(html) = &html {
        let base = first_tag_attribute(&html.document, "base", "href")
            .and_then(|href| resolve_url(html.source_url.as_deref(), &href))
            .or_else(|| html.source_url.as_deref().and_then(normalize_url));
        if let Some(url) = first_tag_attribute(&html.fragment, "img", "src")
            .and_then(|src| resolve_url(base.as_deref(), &src))
        {
            return Some(url);
        }
    }
    payloads
        .locator_wide
        .as_deref()
        .and_then(normalize_url)
        .or_else(|| payloads.locator_ansi.as_deref().and_then(normalize_url))
        .or_else(|| payloads.uri_list.as_deref().and_then(first_uri))
        .or_else(|| payloads.moz_url.as_deref().and_then(moz_url))
        .or_else(|| {
            html.as_ref()
                .and_then(|html| html.source_url.as_deref())
                .and_then(normalize_url)
        })
}

/// A file name for an encoded PNG that arrived without one.
///
/// Uses the URL's last path segment (sanitised, extension replaced by
/// `.png`) so the import label means something, else `dropped.png`.
pub fn png_name_from_url(url: Option<&str>) -> String {
    let segment = url
        .and_then(normalize_url)
        .and_then(|url| {
            let (_, rest) = url.split_once("://")?;
            let path_start = rest.find('/')?;
            let path = &rest[path_start..];
            let path = &path[..path.find(['?', '#']).unwrap_or(path.len())];
            path.rsplit('/').next().map(str::to_owned)
        })
        .unwrap_or_default();
    let stem = segment
        .rsplit_once('.')
        .map_or(segment.as_str(), |(stem, _)| stem);
    let clean: String = stem
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(64)
        .collect();
    let clean = clean.trim_matches('.');
    if clean.is_empty() {
        "dropped.png".to_owned()
    } else {
        format!("{clean}.png")
    }
}

// ---------------------------------------------------------------------------
// Provenance records
// ---------------------------------------------------------------------------

/// What one import knows about its source.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportProvenance {
    /// The file the bytes were read from, when there was one. A browser drag
    /// can deliver a temporary cache file here.
    pub source_path: Option<String>,
    /// The validated source URL, when the drag or paste exposed one.
    pub source_url: Option<String>,
}

/// The provenance one import records, before any dedup merge.
///
/// A URL wins: when a browser hands over both a cache file and a URL, the
/// path is noise that disappears, so it is not recorded.
pub fn record_for(import: &ImportProvenance) -> AssetProvenance {
    let source_url = import.source_url.as_deref().and_then(normalize_url);
    let source_path = if source_url.is_some() {
        None
    } else {
        import.source_path.clone()
    };
    AssetProvenance {
        origin: origin_for(source_path.as_deref(), source_url.as_deref(), None),
        source_path,
        source_url,
    }
}

/// Merges a new import's provenance into an asset's existing record.
///
/// Assets are content-addressed, so one asset can be seen from several
/// sources. The rule: keep an existing path or URL, fill a missing field
/// from the newer import, and recompute `origin` from the result. Because
/// [`record_for`] never records a path alongside a URL, a later browser
/// cache path cannot displace a URL.
pub fn merge(existing: Option<&AssetProvenance>, new: AssetProvenance) -> AssetProvenance {
    let Some(existing) = existing else {
        return new;
    };
    let source_url = existing
        .source_url
        .clone()
        .filter(|url| is_launchable(url))
        .or(new.source_url)
        .or_else(|| existing.source_url.clone());
    let source_path = existing.source_path.clone().or(new.source_path);
    let fallback = match existing.origin {
        AssetOrigin::Unknown => new.origin,
        origin => origin,
    };
    AssetProvenance {
        origin: origin_for(
            source_path.as_deref(),
            source_url.as_deref(),
            Some(fallback),
        ),
        source_path,
        source_url,
    }
}

/// `File` for a durable path without a URL, `Clipboard` for a URL-backed or
/// path-less image, unless a known fallback says otherwise for the latter.
fn origin_for(
    source_path: Option<&str>,
    source_url: Option<&str>,
    fallback: Option<AssetOrigin>,
) -> AssetOrigin {
    match (source_path, source_url) {
        (_, Some(_)) => AssetOrigin::Clipboard,
        (Some(_), None) => AssetOrigin::File,
        (None, None) => match fallback {
            Some(AssetOrigin::File) | None => AssetOrigin::Clipboard,
            Some(origin) => origin,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A CF_HTML header; every offset is ten digits, so its length does not
    /// depend on the values.
    fn header(offsets: [usize; 4], source: &str) -> String {
        let [start_html, end_html, start_fragment, end_fragment] = offsets;
        format!(
            "Version:0.9
StartHTML:{start_html:010}
EndHTML:{end_html:010}
StartFragment:{start_fragment:010}
EndFragment:{end_fragment:010}
{source}"
        )
    }

    /// Builds a CF_HTML payload around `document`, whose fragment is the
    /// first occurrence of `fragment`.
    fn cf_html_document(document: &str, fragment: &str, source_url: Option<&str>) -> Vec<u8> {
        let source = source_url
            .map(|url| {
                format!(
                    "SourceURL:{url}
"
                )
            })
            .unwrap_or_default();
        let header_len = header([0; 4], &source).len();
        let start_fragment = header_len + document.find(fragment).expect("fragment");
        let offsets = [
            header_len,
            header_len + document.len(),
            start_fragment,
            start_fragment + fragment.len(),
        ];
        format!("{}{document}", header(offsets, &source)).into_bytes()
    }

    /// Builds a CF_HTML payload with correct offsets, as Chrome writes it.
    fn cf_html(fragment: &str, source_url: Option<&str>) -> Vec<u8> {
        let document = format!(
            "<html><body>
<!--StartFragment-->{fragment}<!--EndFragment-->
</body></html>"
        );
        cf_html_document(&document, fragment, source_url)
    }

    #[test]
    fn only_http_and_https_urls_survive_normalisation() {
        assert_eq!(
            normalize_url("  \"HTTPS://Example.com/a%20b.png\"  ").as_deref(),
            Some("https://Example.com/a%20b.png"),
            "scheme lowercased, quotes and whitespace trimmed, encoding kept"
        );
        assert_eq!(
            normalize_url("http://host:8080/x").as_deref(),
            Some("http://host:8080/x")
        );
        assert_eq!(
            normalize_url("https://[::1]:443/x").as_deref(),
            Some("https://[::1]:443/x")
        );
        for hostile in [
            "javascript:alert(1)",
            "JaVaScRiPt://example.com/%0aalert(1)",
            "file:///C:/Windows/System32/calc.exe",
            "data:image/png;base64,AAAA",
            "ftp://example.com/x.png",
            "ms-settings:display",
            "\\\\server\\share\\x.png",
            "https://",
            "https:///path",
            "https://user@/x",
            "https://:80/x",
            "https://exa mple.com/x",
            "https://example.com/\u{0}x",
            "https://example.com/\nx",
            "https://example.com\\@evil.com/",
            "//example.com/x.png",
            "example.com/x.png",
            "",
        ] {
            assert_eq!(normalize_url(hostile), None, "{hostile:?} must be refused");
        }
    }

    #[test]
    fn overlong_urls_are_refused_not_truncated() {
        let base = "https://example.com/";
        let fits = format!("{base}{}", "a".repeat(MAX_URL_BYTES - base.len()));
        let over = format!("{fits}a");
        assert!(normalize_url(&fits).is_some());
        assert_eq!(normalize_url(&over), None);
    }

    #[test]
    fn relative_references_resolve_against_the_base() {
        let base = Some("https://example.com/gallery/page.html?id=3#top");
        let cases = [
            ("img/a.png", "https://example.com/gallery/img/a.png"),
            ("./a.png", "https://example.com/gallery/a.png"),
            ("../a.png", "https://example.com/a.png"),
            ("../../../a.png", "https://example.com/a.png"),
            ("/static/a.png?v=2", "https://example.com/static/a.png?v=2"),
            ("//cdn.example.net/a.png", "https://cdn.example.net/a.png"),
            ("?page=2", "https://example.com/gallery/page.html?page=2"),
            ("#frag", "https://example.com/gallery/page.html?id=3#frag"),
            ("a%20b.png", "https://example.com/gallery/a%20b.png"),
            ("http://other.org/x.jpg", "http://other.org/x.jpg"),
            ("img/./b/../c.png", "https://example.com/gallery/img/c.png"),
        ];
        for (reference, expected) in cases {
            assert_eq!(
                resolve_url(base, reference).as_deref(),
                Some(expected),
                "{reference}"
            );
        }
        assert_eq!(
            resolve_url(Some("https://example.com"), "a.png").as_deref(),
            Some("https://example.com/a.png")
        );
    }

    #[test]
    fn a_relative_reference_needs_a_web_base() {
        assert_eq!(resolve_url(None, "a.png"), None);
        assert_eq!(resolve_url(Some("file:///C:/x/"), "a.png"), None);
        assert_eq!(resolve_url(Some("https://e.com/"), "javascript:x"), None);
        assert_eq!(
            resolve_url(Some("https://e.com/"), "data:image/png,x"),
            None
        );
        assert_eq!(resolve_url(Some("https://e.com/"), "   "), None);
    }

    #[test]
    fn cf_html_offsets_and_source_url_are_read() {
        let payload = cf_html(
            r#"<img src="https://img.example.com/cat.png" alt="cat">"#,
            Some("https://example.com/cats"),
        );
        let parsed = parse_cf_html(&payload).expect("well-formed");
        assert_eq!(
            parsed.source_url.as_deref(),
            Some("https://example.com/cats")
        );
        assert!(parsed.fragment.starts_with("<img"));
        assert!(parsed.fragment.ends_with("alt=\"cat\">"));
        assert_eq!(
            url_from_cf_html(&payload).as_deref(),
            Some("https://img.example.com/cat.png")
        );
    }

    #[test]
    fn the_image_src_wins_and_resolves_against_the_source_url() {
        let payload = cf_html(
            "<IMG\r\n  class=hero SRC='../img/dog.jpg?w=800&amp;h=600'>",
            Some("https://example.com/pets/dogs/index.html"),
        );
        assert_eq!(
            url_from_cf_html(&payload).as_deref(),
            Some("https://example.com/pets/img/dog.jpg?w=800&h=600")
        );
    }

    #[test]
    fn a_base_href_outside_the_fragment_is_honoured() {
        let fragment = r#"<img src="a.png">"#;
        let document = format!(
            "<html><head><base href=\"https://cdn.example.com/assets/\"></head><body><!--StartFragment-->{fragment}<!--EndFragment--></body></html>"
        );
        let payload = cf_html_document(&document, fragment, Some("https://example.com/page"));
        assert_eq!(
            url_from_cf_html(&payload).as_deref(),
            Some("https://cdn.example.com/assets/a.png")
        );
    }

    #[test]
    fn without_an_image_the_page_url_is_the_fallback() {
        let payload = cf_html("<p>just text</p>", Some("https://example.com/article"));
        assert_eq!(
            url_from_cf_html(&payload).as_deref(),
            Some("https://example.com/article")
        );
        let data = cf_html(
            r#"<img src="data:image/png;base64,iVBORw0KGgo=">"#,
            Some("https://example.com/p"),
        );
        assert_eq!(
            url_from_cf_html(&data).as_deref(),
            Some("https://example.com/p"),
            "a data: src is ignored in favour of the page"
        );
        let hostile = cf_html(r#"<img src="javascript:alert(1)">"#, Some("file:///C:/x"));
        assert_eq!(url_from_cf_html(&hostile), None);
    }

    #[test]
    fn malformed_cf_html_headers_yield_no_url() {
        let good = cf_html(r#"<img src="https://e.com/a.png">"#, None);
        let text = String::from_utf8(good.clone()).expect("ascii");

        let reversed = text.replacen("StartFragment:", "StartFragment:9", 1);
        assert_eq!(url_from_cf_html(reversed.as_bytes()), None);

        let missing = text.replacen("EndFragment:", "Xnd:", 1);
        assert_eq!(url_from_cf_html(missing.as_bytes()), None);

        let negative = text.replacen("StartFragment:0", "StartFragment:-", 1);
        assert_eq!(url_from_cf_html(negative.as_bytes()), None);

        let truncated = &good[..good.len() - 40];
        assert_eq!(
            url_from_cf_html(truncated),
            None,
            "EndFragment past the end"
        );

        assert_eq!(url_from_cf_html(b"Version:0.9\r\n"), None);
        assert_eq!(url_from_cf_html(b""), None);
        assert_eq!(url_from_cf_html(&[0xFF; 64]), None);
    }

    #[test]
    fn trailing_nul_padding_is_tolerated() {
        let mut payload = cf_html(r#"<img src="https://e.com/a.png">"#, None);
        payload.extend_from_slice(&[0, 0, 0]);
        assert_eq!(
            url_from_cf_html(&payload).as_deref(),
            Some("https://e.com/a.png")
        );
    }

    #[test]
    fn oversized_html_is_refused() {
        let mut payload = b"<img src=\"https://e.com/a.png\">".to_vec();
        payload.resize(MAX_HTML_BYTES + 1, b' ');
        assert_eq!(parse_cf_html(&payload), None);
    }

    #[test]
    fn a_bare_html_fragment_still_yields_its_image() {
        assert_eq!(
            url_from_cf_html(b"<div><img alt=x src=https://e.com/b.webp></div>").as_deref(),
            Some("https://e.com/b.webp")
        );
    }

    #[test]
    fn attribute_scanning_is_tolerant_and_exact() {
        assert_eq!(
            first_tag_attribute(
                r#"<image src="no"><img data-src="no" src="yes">"#,
                "img",
                "src"
            )
            .as_deref(),
            Some("yes"),
            "<image> is not <img>, data-src is not src"
        );
        assert_eq!(
            first_tag_attribute("<img alt='it\"s' src = \"x\" >", "img", "src").as_deref(),
            Some("x")
        );
        assert_eq!(
            first_tag_attribute("<img src=\"unterminated", "img", "src"),
            None
        );
        assert_eq!(first_tag_attribute("<img>", "img", "src"), None);
        assert_eq!(first_tag_attribute("<img src", "img", "src"), None);
        assert_eq!(first_tag_attribute("<img src=", "img", "src"), None);
        assert_eq!(
            first_tag_attribute("<img src>  <img src='b'>", "img", "src").as_deref(),
            Some("b"),
            "a valueless src is skipped; the first image with a value wins"
        );
    }

    #[test]
    fn entities_decode_and_unknown_ones_stay_literal() {
        assert_eq!(decode_entities("a&amp;b&lt;&gt;&quot;&apos;"), "a&b<>\"'");
        assert_eq!(decode_entities("&#39;&#x41;&#X42;"), "'AB");
        assert_eq!(decode_entities("&nbsp;&bogus;&"), "&nbsp;&bogus;&");
        assert_eq!(decode_entities("&#xFFFFFFFF;"), "&#xFFFFFFFF;");
        assert_eq!(decode_entities("a & b; c"), "a & b; c");
    }

    #[test]
    fn uri_lists_skip_comments_and_invalid_lines() {
        assert_eq!(
            first_uri("# from the browser\r\nfile:///tmp/x\r\nhttps://e.com/a.png\r\n").as_deref(),
            Some("https://e.com/a.png")
        );
        assert_eq!(first_uri("# only a comment\n"), None);
        assert_eq!(first_uri(""), None);
    }

    #[test]
    fn moz_url_is_the_first_line() {
        assert_eq!(
            moz_url("https://e.com/a.png\nA cat").as_deref(),
            Some("https://e.com/a.png")
        );
        assert_eq!(moz_url("A cat\nhttps://e.com/a.png"), None);
    }

    #[test]
    fn wide_and_narrow_strings_stop_at_the_terminator() {
        let mut wide: Vec<u8> = "https://e.com/ä.png"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        wide.extend_from_slice(&[0, 0, b'x', 0]);
        assert_eq!(utf16_until_nul(&wide), "https://e.com/ä.png");
        assert_eq!(
            utf16_until_nul(&[0x68]),
            "",
            "an odd trailing byte is dropped"
        );
        assert_eq!(bytes_until_nul(b"https://e.com/a\0junk"), "https://e.com/a");
        assert_eq!(bytes_until_nul(b"no terminator"), "no terminator");

        let moz: Vec<u8> = "https://e.com/a.png\nA cat\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            moz_url(&text_payload(&moz)).as_deref(),
            Some("https://e.com/a.png")
        );
        assert_eq!(
            text_payload(b"https://e.com/b\r\n\0"),
            "https://e.com/b\r\n"
        );
        assert_eq!(text_payload(b""), "");
    }

    #[test]
    fn a_drop_prefers_the_image_src_then_the_url_formats() {
        let html = cf_html(
            r#"<a href="https://e.com/post"><img src="/i/full.jpg"></a>"#,
            Some("https://e.com/feed"),
        );
        let all = DropUrlPayloads {
            locator_wide: Some("https://e.com/post".to_owned()),
            locator_ansi: Some("https://e.com/ansi".to_owned()),
            uri_list: Some("https://e.com/list".to_owned()),
            moz_url: Some("https://e.com/moz\ntitle".to_owned()),
            html: Some(html),
        };
        assert_eq!(
            choose_drop_url(&all).as_deref(),
            Some("https://e.com/i/full.jpg")
        );

        let mut no_img = all.clone();
        no_img.html = Some(cf_html("<p>x</p>", Some("https://e.com/feed")));
        assert_eq!(
            choose_drop_url(&no_img).as_deref(),
            Some("https://e.com/post")
        );
        no_img.locator_wide = Some("javascript:alert(1)".to_owned());
        assert_eq!(
            choose_drop_url(&no_img).as_deref(),
            Some("https://e.com/ansi")
        );
        no_img.locator_ansi = None;
        assert_eq!(
            choose_drop_url(&no_img).as_deref(),
            Some("https://e.com/list")
        );
        no_img.uri_list = None;
        assert_eq!(
            choose_drop_url(&no_img).as_deref(),
            Some("https://e.com/moz")
        );
        no_img.moz_url = None;
        assert_eq!(
            choose_drop_url(&no_img).as_deref(),
            Some("https://e.com/feed"),
            "the page SourceURL is the last resort"
        );
        assert_eq!(choose_drop_url(&DropUrlPayloads::default()), None);
    }

    #[test]
    fn png_fallback_names_come_from_the_url() {
        assert_eq!(
            png_name_from_url(Some("https://e.com/a/b/Cat%20Photo.JPG?x=1#y")),
            "Cat20Photo.png"
        );
        assert_eq!(png_name_from_url(Some("https://e.com/hero")), "hero.png");
        assert_eq!(png_name_from_url(Some("https://e.com/")), "dropped.png");
        assert_eq!(png_name_from_url(Some("https://e.com")), "dropped.png");
        assert_eq!(png_name_from_url(Some("javascript:x")), "dropped.png");
        assert_eq!(
            png_name_from_url(Some("https://e.com/../..")),
            "dropped.png"
        );
        assert_eq!(png_name_from_url(None), "dropped.png");
        let long = format!("https://e.com/{}.png", "x".repeat(300));
        assert_eq!(png_name_from_url(Some(&long)).len(), 64 + ".png".len());
    }

    fn import(path: Option<&str>, url: Option<&str>) -> ImportProvenance {
        ImportProvenance {
            source_path: path.map(str::to_owned),
            source_url: url.map(str::to_owned),
        }
    }

    #[test]
    fn a_url_suppresses_the_browser_cache_path() {
        let record = record_for(&import(
            Some(r"C:\Users\a\AppData\Local\Temp\chrome_drag1234\cat.png"),
            Some("https://e.com/cat.png"),
        ));
        assert_eq!(record.source_path, None);
        assert_eq!(record.source_url.as_deref(), Some("https://e.com/cat.png"));
        assert_eq!(record.origin, AssetOrigin::Clipboard);
    }

    #[test]
    fn origins_follow_the_plan() {
        assert_eq!(
            record_for(&import(Some(r"D:\refs\a.png"), None)).origin,
            AssetOrigin::File
        );
        assert_eq!(
            record_for(&import(None, None)).origin,
            AssetOrigin::Clipboard
        );
        let invalid = record_for(&import(Some(r"D:\refs\a.png"), Some("javascript:x")));
        assert_eq!(invalid.source_url, None, "an invalid URL is not recorded");
        assert_eq!(invalid.source_path.as_deref(), Some(r"D:\refs\a.png"));
        assert_eq!(invalid.origin, AssetOrigin::File);
    }

    #[test]
    fn the_merge_keeps_the_best_provenance() {
        let web = record_for(&import(
            Some(r"C:\Temp\cache.png"),
            Some("https://e.com/a.png"),
        ));
        let disk = record_for(&import(Some(r"D:\refs\a.png"), None));
        let pasted = record_for(&import(None, None));

        // A later cache path never displaces a URL.
        let later_cache = record_for(&import(Some(r"C:\Temp\other.png"), Some("https://x.com/b")));
        let merged = merge(Some(&web), later_cache);
        assert_eq!(merged.source_url.as_deref(), Some("https://e.com/a.png"));
        assert_eq!(merged.source_path, None);

        // A durable path over a clipboard paste.
        let merged = merge(Some(&pasted), disk.clone());
        assert_eq!(merged.source_path.as_deref(), Some(r"D:\refs\a.png"));
        assert_eq!(merged.origin, AssetOrigin::File);
        let merged = merge(Some(&disk), pasted.clone());
        assert_eq!(merged.source_path.as_deref(), Some(r"D:\refs\a.png"));
        assert_eq!(merged.origin, AssetOrigin::File);

        // Missing fields fill from the newer import; the URL decides origin.
        let merged = merge(Some(&disk), web.clone());
        assert_eq!(merged.source_path.as_deref(), Some(r"D:\refs\a.png"));
        assert_eq!(merged.source_url.as_deref(), Some("https://e.com/a.png"));
        assert_eq!(merged.origin, AssetOrigin::Clipboard);

        // No existing record: the new one stands.
        assert_eq!(merge(None, web.clone()), web);
    }

    #[test]
    fn the_merge_respects_records_from_older_boards() {
        let generated = AssetProvenance {
            origin: AssetOrigin::Generated,
            source_path: None,
            source_url: None,
        };
        let pasted = record_for(&import(None, None));
        assert_eq!(
            merge(Some(&generated), pasted.clone()).origin,
            AssetOrigin::Generated
        );

        let unknown = AssetProvenance::default();
        assert_eq!(merge(Some(&unknown), pasted).origin, AssetOrigin::Clipboard);

        // A hand-edited board with a non-web URL: a real URL replaces it,
        // but it is kept when nothing better arrives.
        let hostile = AssetProvenance {
            origin: AssetOrigin::Clipboard,
            source_path: None,
            source_url: Some("javascript:alert(1)".to_owned()),
        };
        let web = record_for(&import(None, Some("https://e.com/a.png")));
        assert_eq!(
            merge(Some(&hostile), web).source_url.as_deref(),
            Some("https://e.com/a.png")
        );
        assert_eq!(
            merge(Some(&hostile), record_for(&import(None, None)))
                .source_url
                .as_deref(),
            Some("javascript:alert(1)")
        );
    }

    #[test]
    fn plain_clipboard_html_yields_only_absolute_web_image_urls() {
        use super::url_from_plain_html;
        assert_eq!(
            url_from_plain_html(r#"<meta charset="utf-8"><img src="https://e.com/a.png" alt="">"#)
                .as_deref(),
            Some("https://e.com/a.png")
        );
        assert_eq!(
            url_from_plain_html(r#"<base href="https://e.com/dir/"><img src="b.jpg">"#).as_deref(),
            Some("https://e.com/dir/b.jpg")
        );
        assert_eq!(url_from_plain_html(r#"<img src="relative.png">"#), None);
        assert_eq!(
            url_from_plain_html(r#"<img src="file:///etc/passwd">"#),
            None
        );
        assert_eq!(url_from_plain_html("<p>no image</p>"), None);
    }
}
