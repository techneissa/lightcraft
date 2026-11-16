//! XMP packets (ISO 16684-1 / Adobe XMP Specification Part 1–2): read and write via `craft-metadata`.

use crate::Metadata;
use crate::convert::{to_craft, xmp_from_craft};
pub use craft_metadata::{CRS_NS, XmpError, XmpNamespace, XmpValue, XmpWrite, parse_xmp_with, write_xmp_with};
use std::collections::BTreeMap;

/// LightCraft's XMP namespace URI (prefix `lc`).
pub const LC_NS: &str = "http://ns.lightcraft.app/lc/1.0/";

const LC_NAMESPACES: &[XmpNamespace<'static>] = &[XmpNamespace { prefix: "lc", uri: LC_NS }];

/// Result of [`parse_xmp`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct XmpData {
    pub metadata: Metadata,
    /// The opaque `lc:settings` JSON string, if present.
    pub lc_settings: Option<String>,
    /// Every property found, keyed `prefix:name`.
    pub properties: BTreeMap<String, Vec<String>>,
    /// Every top-level property with its full structure.
    pub values: BTreeMap<String, XmpValue>,
}

impl XmpData {
    /// The pixel size the packet's regions were drawn on (`mwg-rs:AppliedToDimensions`), unless one of them says how
    /// the photo was turned (`mwg-rs:Rotation`).
    pub fn regions_drawn_on(&self) -> Option<(f64, f64)> {
        let regions = self.values.get("mwg-rs:Regions")?;
        let turned = regions.field("mwg-rs:RegionList").is_some_and(|l| l.items().iter().any(|r| r.field("mwg-rs:Rotation").is_some()));
        if turned { None } else { applied_to(regions) }
    }
}

fn applied_to(regions: &XmpValue) -> Option<(f64, f64)> {
    let dims = regions.field("mwg-rs:AppliedToDimensions")?;
    let w = dims.field("stDim:w")?.text()?.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)?;
    let h = dims.field("stDim:h")?.text()?.parse::<f64>().ok().filter(|v| v.is_finite() && *v > 0.0)?;
    let unit = dims.field("stDim:unit").and_then(XmpValue::text).unwrap_or("pixel");
    (unit.is_empty() || unit.eq_ignore_ascii_case("pixel")).then_some((w, h))
}

pub fn parse_xmp(s: &str) -> Result<XmpData, XmpError> {
    parse_xmp_with(s, LC_NAMESPACES).map(xmp_from_craft)
}

pub fn write_xmp(meta: &Metadata, lc_settings: Option<&str>) -> String {
    let settings = lc_settings.map(|s| [("settings", s)]);
    write_xmp_lc(meta, settings.as_ref().map_or(&[], |a| a.as_slice()))
}

pub fn write_xmp_lc(meta: &Metadata, lc: &[(&str, &str)]) -> String {
    let x = write_xmp_with(
        &to_craft(meta),
        &XmpWrite { toolkit: "LightCraft", namespace: Some(XmpNamespace { prefix: "lc", uri: LC_NS }), properties: lc },
    );
    with_fork_tags(x, meta)
}

/// Fork (`fork/metadata-overrides.md`): the lens and the sensitivity are also written in their second standard
/// places, `aux:Lens` and `exifEX:PhotographicSensitivity` (other programs read one or the other), inside the
/// packet's description. The shared writer declares both namespaces.
fn with_fork_tags(mut x: String, meta: &Metadata) -> String {
    let mut extra = String::new();
    let mut push = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            extra.push_str(&format!("   <{k}>{}</{k}>\n", xml_text(&v)));
        }
    };
    push("aux:Lens", meta.lens_model.clone());
    push("exifEX:PhotographicSensitivity", meta.iso.filter(|v| *v > 0).map(|v| v.to_string()));
    if extra.is_empty() {
        return x;
    }
    if let Some(at) = x.rfind("  </rdf:Description>") {
        x.insert_str(at, &extra);
    }
    x
}

/// Text for an XML element: characters XML 1.0 can't hold are left out, and markup is escaped.
fn xml_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => out.push(c),
        }
    }
    out
}
