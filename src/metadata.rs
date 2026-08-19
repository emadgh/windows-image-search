use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

#[derive(Default, Debug)]
pub struct TextMetadata {
    pub description: String,
    pub keywords: String,
}

pub fn extract(path: &Path) -> TextMetadata {
    let mut descriptions = Vec::new();
    let mut keywords = Vec::new();

    if let Ok(file) = File::open(path) {
        let mut reader = BufReader::new(file);
        if let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) {
            for field in exif.fields() {
                let tag = field.tag.to_string().to_ascii_lowercase();
                let value = field.display_value().with_unit(&exif).to_string();
                let value = clean_text(&value);
                if value.is_empty() {
                    continue;
                }
                if tag.contains("keyword") || tag.contains("subject") {
                    keywords.push(value.clone());
                }
                if tag.contains("description")
                    || tag.contains("comment")
                    || tag.contains("title")
                    || tag.contains("caption")
                {
                    descriptions.push(value);
                }
            }
        }
    }

    // XMP packets in JPEG/PNG/TIFF are typically UTF-8 text. This lightweight
    // fallback extracts the common Dublin Core / XMP fields without requiring
    // a platform-native metadata library.
    if let Ok(mut file) = File::open(path) {
        let mut bytes = Vec::new();
        let _ = file.by_ref().take(4 * 1024 * 1024).read_to_end(&mut bytes);
        let text = String::from_utf8_lossy(&bytes);

        for tag in ["dc:description", "dc:title", "photoshop:Headline"] {
            descriptions.extend(extract_xml_elements(&text, tag));
        }
        for tag in ["dc:subject", "photoshop:Keywords"] {
            keywords.extend(extract_xml_elements(&text, tag));
        }
        for attribute in ["xmp:Label", "photoshop:Headline"] {
            if let Some(value) = extract_xml_attribute(&text, attribute) {
                descriptions.push(value);
            }
        }
    }

    normalize_values(&mut descriptions);
    normalize_values(&mut keywords);

    TextMetadata {
        description: descriptions.join(" | "),
        keywords: keywords.join(", "),
    }
}

fn extract_xml_elements(text: &str, tag: &str) -> Vec<String> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut cursor = 0;

    while let Some(start_rel) = text[cursor..].find(&open) {
        let start = cursor + start_rel;
        let Some(open_end_rel) = text[start..].find('>') else {
            break;
        };
        let content_start = start + open_end_rel + 1;
        let Some(close_rel) = text[content_start..].find(&close) else {
            break;
        };
        let content_end = content_start + close_rel;
        let value = clean_text(&strip_xml_tags(&text[content_start..content_end]));
        if !value.is_empty() {
            out.push(value);
        }
        cursor = content_end + close.len();
        if cursor >= text.len() {
            break;
        }
    }
    out
}

fn extract_xml_attribute(text: &str, attribute: &str) -> Option<String> {
    let needle = format!("{attribute}=\"");
    let start = text.find(&needle)? + needle.len();
    let end = text[start..].find('"')? + start;
    let value = clean_text(&text[start..end]);
    (!value.is_empty()).then_some(value)
}

fn strip_xml_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn clean_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_values(values: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| {
        let normalized = value.trim().to_ascii_lowercase();
        !normalized.is_empty() && seen.insert(normalized)
    });
}
