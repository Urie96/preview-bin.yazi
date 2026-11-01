//! Minimal EPUB inspection: unzip into a temp dir, locate the OPF package
//! document, then pull metadata and the cover image out of it.
//!
//! The original bash version shelled out to `grep`/`sed` for this; here the
//! small amount of XML we care about is parsed by hand so there is no extra
//! dependency.

use std::fs;
use std::path::{Path, PathBuf};

use crate::sys;

pub struct Epub {
    dir: PathBuf,
    opf: Option<PathBuf>,
    opf_text: String,
}

impl Epub {
    pub fn open(path: &Path) -> Result<Self, String> {
        let unzip = sys::which("unzip").ok_or_else(|| "unzip: command not found".to_string())?;
        let dir = sys::temp_path("epub.d");
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

        let args = vec![
            sys::os("-q"),
            path.as_os_str().to_owned(),
            sys::os("-d"),
            dir.as_os_str().to_owned(),
        ];
        if let Err(err) = sys::status_of(&unzip, &args) {
            let _ = fs::remove_dir_all(&dir);
            return Err(err);
        }

        let opf = find_file(&dir, |p| {
            p.extension()
                .map(|e| e.to_string_lossy().eq_ignore_ascii_case("opf"))
                .unwrap_or(false)
        });
        let opf_text = opf
            .as_ref()
            .and_then(|p| fs::read(p).ok())
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();

        Ok(Self { dir, opf, opf_text })
    }

    pub fn title(&self) -> Option<String> {
        element_text(&self.opf_text, "dc:title")
    }

    pub fn creator(&self) -> Option<String> {
        element_text(&self.opf_text, "dc:creator")
    }

    pub fn language(&self) -> Option<String> {
        element_text(&self.opf_text, "dc:language")
    }

    pub fn description(&self) -> Option<String> {
        element_text(&self.opf_text, "dc:description")
    }

    pub fn cover(&self) -> Option<PathBuf> {
        if let (Some(opf), Some(href)) = (self.opf.as_ref(), self.cover_href()) {
            let candidate = opf.parent().unwrap_or_else(|| Path::new("")).join(href);
            if candidate.is_file() {
                return Some(candidate);
            }
        }

        for name in [
            "cover.jpg",
            "cover.jpeg",
            "cover.png",
            "cover.gif",
            "title.jpg",
            "title.png",
        ] {
            let found = find_file(&self.dir, |p| {
                p.file_name()
                    .map(|f| f.to_string_lossy().eq_ignore_ascii_case(name))
                    .unwrap_or(false)
            });
            if found.is_some() {
                return found;
            }
        }

        find_file(&self.dir, is_image)
    }

    /// Resolve the manifest item marked as the cover image to its `href`.
    fn cover_href(&self) -> Option<String> {
        let text = &self.opf_text;

        let mut cover_id = None;
        for tag in text.split('<') {
            if !is_item_tag(tag) {
                continue;
            }
            let has_cover_property = attr(tag, "properties")
                .map(|props| props.split_whitespace().any(|p| p == "cover-image"))
                .unwrap_or(false);
            if has_cover_property {
                cover_id = attr(tag, "id");
                break;
            }
        }

        if cover_id.is_none() {
            for tag in text.split('<') {
                if !is_item_tag(tag) {
                    continue;
                }
                if let Some(id) = attr(tag, "id") {
                    if id.to_ascii_lowercase().contains("cover") {
                        cover_id = Some(id);
                        break;
                    }
                }
            }
        }

        let cover_id = cover_id?;
        for tag in text.split('<') {
            if is_item_tag(tag) && attr(tag, "id").as_deref() == Some(cover_id.as_str()) {
                return attr(tag, "href");
            }
        }
        None
    }
}

impl Drop for Epub {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn is_item_tag(tag: &str) -> bool {
    tag.starts_with("item")
        && tag[4..]
            .chars()
            .next()
            .map(|c| c.is_whitespace())
            .unwrap_or(false)
}

/// Read an XML attribute value, requiring an attribute-name boundary.
fn attr(tag: &str, key: &str) -> Option<String> {
    let bytes = tag.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = tag[search..].find(key) {
        let at = search + rel;
        let boundary = at == 0
            || matches!(
                bytes[at - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'<' | b'"' | b'\''
            );
        let after_key = at + key.len();
        if boundary && tag[after_key..].starts_with("=\"") {
            let start = after_key + 2;
            let rest = &tag[start..];
            let end = rest.find('"')?;
            return Some(decode_entities(&rest[..end]));
        }
        search = after_key;
    }
    None
}

/// Text content of the first `<tag ...>...</tag>` element.
fn element_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut search = 0usize;

    while let Some(rel) = xml[search..].find(&open) {
        let at = search + rel;
        let after = xml[at + open.len()..].chars().next();
        let valid_after = matches!(
            after,
            Some('>') | Some(' ') | Some('\t') | Some('\n') | Some('\r') | Some('/')
        );
        if !valid_after {
            search = at + open.len();
            continue;
        }

        let gt = at + xml[at..].find('>')? + 1;
        if xml[at..gt].trim_end().ends_with("/>") {
            search = gt;
            continue;
        }

        let rest = &xml[gt..];
        let close = format!("</{tag}");
        let end = rest.find(&close)?;
        let value = rest[..end].trim();
        if value.is_empty() {
            return None;
        }
        return Some(decode_entities(value));
    }
    None
}

fn decode_entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .map(|e| {
            matches!(
                e.to_string_lossy().to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp"
            )
        })
        .unwrap_or(false)
}

/// Depth-first search for the first file matching `predicate`.
fn find_file<F>(root: &Path, predicate: F) -> Option<PathBuf>
where
    F: Fn(&Path) -> bool,
{
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).ok()?;
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                stack.push(path);
            } else if predicate(&path) {
                return Some(path);
            }
        }
    }
    None
}
