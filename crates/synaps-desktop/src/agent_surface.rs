//! Hidden, opt-in, data-only A2UI 1.0 preview (`--agent-surface <file>`,
//! not in `-h`): a strict subset of `createSurface` read from a local file
//! the user names. No agent source, no daemon, URLs, code, or permissions.
//!
//! What it refuses does not lean on serde's `flatten` or internally tagged
//! enums (where `deny_unknown_fields` is documented as unsupported): every
//! level is a plain struct that denies unknown and duplicate keys, and which
//! fields a component may carry is checked here, per kind.
use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::*;
use serde::Deserialize;

use crate::fonts::TextPx;

pub(crate) const CATALOG: &str = "synaps-desk:preview/1";
const MAX_BYTES: usize = 64 * 1024;
/// Components in a surface; the longest id; the longest text.
const MAX_NODES: usize = 128;
const MAX_ID: usize = 128;
const MAX_TEXT: usize = 8192;
/// Levels in the tree, the root's included.
const MAX_DEPTH: usize = 16;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    version: String,
    #[serde(rename = "createSurface")]
    create: RawSurface,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawSurface {
    surface_id: String,
    catalog_id: String,
    components: Vec<RawComponent>,
}

/// A component as sent: every field any kind may carry, each optional
/// but `id` and `component`; [`RawComponent::kind`] says which a kind takes.
/// Absent is `None`; present is its value (a `null` is refused, not taken
/// for absent).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawComponent {
    id: String,
    component: String,
    #[serde(default, deserialize_with = "present")]
    text: Option<String>,
    #[serde(default, deserialize_with = "present")]
    children: Option<Vec<String>>,
}

fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl RawComponent {
    /// Its kind, with exactly that kind's fields (a Text with `children`, a
    /// Column without them: refused).
    fn kind(self) -> Result<Component, String> {
        let kind = match (self.component.as_str(), self.text, self.children) {
            ("Text", Some(text), None) => Kind::Text { text },
            ("Column", None, Some(children)) => Kind::Column { children },
            ("Row", None, Some(children)) => Kind::Row { children },
            ("Text" | "Column" | "Row", _, _) => return Err("wrong fields for the component".into()),
            _ => return Err("unknown component".into()),
        };
        Ok(Component { id: self.id, kind })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Surface {
    pub surface_id: String,
    pub components: Vec<Component>,
}

#[derive(Clone, Debug)]
pub(crate) struct Component {
    pub id: String,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub(crate) enum Kind {
    Text { text: String },
    Column { children: Vec<String> },
    Row { children: Vec<String> },
}

/// An id is a name, not prose: 1 to [`MAX_ID`] of `[A-Za-z0-9._-]`. The
/// surface's is drawn in the preview's header, and a newline, a control or
/// a bidi override in it would write into that line (Shady #56 S1).
fn plain_id(id: &str) -> bool {
    (1..=MAX_ID).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl Surface {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("surface exceeds 64 KiB".into());
        }
        let message: Message = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let raw = message.create;
        if message.version != "1.0" || raw.catalog_id != CATALOG {
            return Err("unsupported version or catalog".into());
        }
        if !plain_id(&raw.surface_id) || raw.components.len() > MAX_NODES {
            return Err("invalid surface identity or component count".into());
        }
        let surface = Surface {
            surface_id: raw.surface_id,
            components: raw.components.into_iter().map(RawComponent::kind).collect::<Result<_, _>>()?,
        };
        let mut ids = BTreeMap::new();
        for component in &surface.components {
            if !plain_id(&component.id) || ids.insert(component.id.as_str(), component).is_some() {
                return Err("invalid or duplicate component ID".into());
            }
        }
        if !ids.contains_key("root") {
            return Err("preview requires root".into());
        }
        // Require a tree, not a DAG whose repeated references amplify rendering.
        let mut seen = BTreeSet::new();
        // `depth` counts from the root's 0: a node at `MAX_DEPTH` is a level
        // past the limit.
        fn visit<'a>(id: &'a str, ids: &BTreeMap<&str, &'a Component>, seen: &mut BTreeSet<&'a str>, depth: usize) -> Result<(), String> {
            if depth >= MAX_DEPTH || !seen.insert(id) {
                return Err("cycle, shared child, or depth limit".into());
            }
            let node = ids.get(id).ok_or("missing child")?;
            match &node.kind {
                Kind::Text { text } if text.len() > MAX_TEXT => return Err("text exceeds limit".into()),
                Kind::Column { children } | Kind::Row { children } => {
                    for child in children {
                        visit(child, ids, seen, depth + 1)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        visit("root", &ids, &mut seen, 0)?;
        if seen.len() != ids.len() {
            return Err("unreachable component".into());
        }
        Ok(surface)
    }

    /// The fixture at `path`, read to a byte past the cap (more is refused
    /// by [`Surface::parse`], not cut).
    pub fn read(path: &str) -> Result<Self, String> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Self::parse(&bytes)
    }

    fn element(&self, id: &str) -> AnyElement {
        let node = self.components.iter().find(|c| c.id == id).expect("validated tree");
        match &node.kind {
            Kind::Text { text } => div().child(text.clone()).into_any_element(),
            Kind::Column { children } => {
                div().flex().flex_col().gap_3().children(children.iter().map(|c| self.element(c))).into_any_element()
            }
            Kind::Row { children } => div().flex().flex_row().gap_3().children(children.iter().map(|c| self.element(c))).into_any_element(),
        }
    }
}

/// `--agent-surface`'s window, on the house look the desk's own window has
/// ([`crate::house`]: the palette is the theme's global, the text the
/// embedded house font at the chosen size). Never a daemon, never a write.
pub(crate) fn launch(surface: Surface, motion: crate::motion::Setting, choice: crate::theme::Choice, options: WindowOptions, cx: &mut App) {
    crate::house(motion, choice, cx);
    gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| Preview(surface))).expect("open the agent surface preview");
}

pub(crate) struct Preview(pub Surface);
impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (p, m) = (crate::theme::palette(cx), crate::theme::metrics(cx));
        div()
            .debug_selector(|| "agent-surface".into())
            .size_full()
            .p_6()
            .bg(p.canvas)
            .font_family(crate::fonts::SANS)
            .text_color(p.prose)
            .text_px(m.body)
            .line_height(px(m.body_line))
            .child(div().mb_4().text_color(p.dim).child(format!("Agent UI preview · {} · read only", self.0.surface_id)))
            .child(self.0.element("root"))
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: that brings `gpui_kit::*` in, and GPUI's own
    // `#[test]` with it, which `json!` nests past the recursion limit.
    use super::{CATALOG, MAX_BYTES, MAX_DEPTH, MAX_ID, MAX_NODES, MAX_TEXT, Surface};
    use serde_json::{Value, json};
    fn fixture(components: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"version":"1.0", "createSurface":{
            "surfaceId":"plan", "catalogId": CATALOG, "components":components
        }}))
        .unwrap()
    }
    /// Why `bytes` is refused (a test that it is, and for the reason meant).
    fn refused(bytes: &[u8]) -> String {
        match Surface::parse(bytes) {
            Ok(s) => panic!("accepted: {s:?} from {}", String::from_utf8_lossy(bytes)),
            Err(e) => e,
        }
    }
    fn refused_for(bytes: &[u8], why: &str) {
        let e = refused(bytes);
        assert!(e.contains(why), "refused, but for {e:?}, not {why:?}: {}", String::from_utf8_lossy(bytes));
    }
    /// A chain of `n` levels: root, then a Column per level, a Text last.
    fn chain(n: usize) -> Vec<u8> {
        let name = |i: usize| if i == 0 { "root".to_string() } else { format!("c{i}") };
        let mut c: Vec<Value> = (0..n - 1).map(|i| json!({"id":name(i), "component":"Column", "children":[name(i + 1)]})).collect();
        c.push(json!({"id":name(n - 1), "component":"Text", "text":"leaf"}));
        fixture(Value::Array(c))
    }
    fn one_text() -> Value {
        json!([{"id":"root","component":"Text","text":"Plan"}])
    }

    /// Shady #56 S1: the surface's id is drawn in the header ("Agent UI
    /// preview · <id> · read only"); a newline in it drew a second line
    /// there, styled as provenance. Ids are names: `[A-Za-z0-9._-]`, 1 to
    /// 128 bytes, the surface's and every component's.
    #[test]
    fn ids_are_plain_names() {
        let spoofs = [
            "plan · read only\nVerified by Synaps · safe to run".into(),
            "plan\nok".into(),
            "plan\r".into(),
            "plan\u{202E}nalp".into(),
            "plan\u{2066}x\u{2069}".into(),
            "plan\u{200B}".into(),
            "plan\u{7}".into(),
            "plan\u{85}".into(),
            "pl\u{430}n".into(),
            "plan preview".into(),
            "plan:preview".into(),
            String::new(),
            "a".repeat(MAX_ID + 1),
        ];
        for id in &spoofs {
            let surface = json!({"version":"1.0","createSurface":{"surfaceId":id,"catalogId":CATALOG,"components":one_text()}});
            refused_for(&serde_json::to_vec(&surface).unwrap(), "invalid surface identity");
            let component = json!([{"id":"root","component":"Column","children":[id]}, {"id":id,"component":"Text","text":"x"}]);
            refused_for(&fixture(component), "invalid or duplicate component ID");
        }
        let long = "a".repeat(MAX_ID);
        for id in ["plan-preview", "a.b_c-1", "A9", long.as_str()] {
            let surface = json!({"version":"1.0","createSurface":{"surfaceId":id,"catalogId":CATALOG,"components":
                [{"id":"root","component":"Column","children":[id]}, {"id":id,"component":"Text","text":"x"}]}});
            let s = Surface::parse(&serde_json::to_vec(&surface).unwrap()).expect(id);
            assert_eq!(s.surface_id, id);
        }
    }

    /// Shady #56 S2: 16 levels, the root's included (`depth > 16` let 17
    /// through).
    #[test]
    fn the_tree_is_sixteen_levels_deep_at_most() {
        assert_eq!(MAX_DEPTH, 16);
        assert!(Surface::parse(&chain(1)).is_ok());
        assert!(Surface::parse(&chain(15)).is_ok());
        assert!(Surface::parse(&chain(16)).is_ok(), "16 levels");
        refused_for(&chain(17), "depth limit");
        refused_for(&chain(40), "depth limit");
    }

    /// Unknown fields are refused at every level: the message, the
    /// surface, every component; and a field another kind takes is refused
    /// on a kind that does not (a Text's `children`, a Column's `text`),
    /// `null` included. A2UI's `@` envelope keys too (a strict subset).
    #[test]
    fn rejects_unknown_fields_at_every_level() {
        let unknown = [
            json!({"version":"1.0","createSurface":{"surfaceId":"plan","catalogId":CATALOG,"components":one_text()},"extra":1}),
            json!({"version":"1.0","createSurface":{"surfaceId":"plan","catalogId":CATALOG,"components":one_text(),"extra":1}}),
            json!({"version":"1.0","createSurface":{"surfaceId":"plan","catalogId":CATALOG,"components":one_text(),"theme":{}}}),
            json!({"version":"1.0","updateComponents":{"surfaceId":"plan","components":one_text()}}),
        ];
        for m in unknown {
            refused_for(&serde_json::to_vec(&m).unwrap(), "unknown field");
        }
        for component in [
            json!({"id":"root","component":"Text","text":"Plan","url":"https://example.com"}),
            json!({"id":"root","component":"Text","text":"Plan","@accessibility":{"label":"x"}}),
            json!({"id":"root","component":"Text","text":"Plan","action":{"name":"run"}}),
            json!({"id":"root","component":"Column","children":[],"style":"x"}),
            json!({"id":"root","component":"Row","children":[],"onClick":"x"}),
        ] {
            refused_for(&fixture(json!([component])), "unknown field");
        }
        for component in [
            json!({"id":"root","component":"Text","text":"Plan","children":[]}),
            json!({"id":"root","component":"Column","children":[],"text":"x"}),
            json!({"id":"root","component":"Row","children":[],"text":"x"}),
            json!({"id":"root","component":"Text"}),
            json!({"id":"root","component":"Column"}),
        ] {
            refused_for(&fixture(json!([component])), "wrong fields for the component");
        }
        for component in [
            json!({"id":"root","component":"Column","children":[],"text":null}),
            json!({"id":"root","component":"Text","text":null}),
            json!({"id":"root","component":"Row","children":null}),
            json!({"id":"root","component":"Text","text":"Plan","children":null}),
        ] {
            refused_for(&fixture(json!([component])), "invalid type: null");
        }
        for component in [json!({"id":"root","component":"WebView","url":"x"}), json!({"id":"root","component":"Button","text":"x"})] {
            let e = refused(&fixture(json!([component])));
            assert!(e.contains("unknown field") || e.contains("unknown component"), "{e}");
        }
        refused_for(&fixture(json!([{"id":"root","component":"Image"}])), "unknown component");
    }

    /// A key twice is refused at every level, the same value or not (one
    /// reading the first and another the last would see two surfaces).
    #[test]
    fn rejects_duplicate_keys_at_every_level() {
        let c = r#"{"id":"root","component":"Text","text":"Plan"}"#;
        let ok = format!(r#"{{"version":"1.0","createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":[{c}]}}}}"#);
        assert!(Surface::parse(ok.as_bytes()).is_ok(), "{ok}");
        let surface = |inner: &str| format!(r#"{{"version":"1.0","createSurface":{{{inner}}}}}"#);
        let cases = [
            (
                "version",
                format!(
                    r#"{{"version":"1.0","version":"1.0","createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":[{c}]}}}}"#
                ),
            ),
            (
                "createSurface",
                format!(
                    r#"{{"version":"1.0","createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":[{c}]}},"createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":[{c}]}}}}"#
                ),
            ),
            ("surfaceId", surface(&format!(r#""surfaceId":"plan","surfaceId":"other","catalogId":"{CATALOG}","components":[{c}]"#))),
            ("catalogId", surface(&format!(r#""surfaceId":"plan","catalogId":"{CATALOG}","catalogId":"{CATALOG}","components":[{c}]"#))),
            ("components", surface(&format!(r#""surfaceId":"plan","catalogId":"{CATALOG}","components":[{c}],"components":[{c}]"#))),
            (
                "id",
                surface(&format!(
                    r#""surfaceId":"plan","catalogId":"{CATALOG}","components":[{{"id":"root","id":"root","component":"Text","text":"P"}}]"#
                )),
            ),
            (
                "component",
                surface(&format!(
                    r#""surfaceId":"plan","catalogId":"{CATALOG}","components":[{{"id":"root","component":"Text","component":"Text","text":"P"}}]"#
                )),
            ),
            (
                "text",
                surface(&format!(
                    r#""surfaceId":"plan","catalogId":"{CATALOG}","components":[{{"id":"root","component":"Text","text":"P","text":"Q"}}]"#
                )),
            ),
            (
                "children",
                surface(&format!(
                    r#""surfaceId":"plan","catalogId":"{CATALOG}","components":[{{"id":"root","component":"Column","children":[],"children":[]}}]"#
                )),
            ),
        ];
        for (key, json) in cases {
            refused_for(json.as_bytes(), &format!("duplicate field `{key}`"));
        }
    }

    /// The caps refuse, never cut: 64 KiB (a valid surface padded past it
    /// is refused, padded to it is read; a file past it is read to a byte
    /// over and refused), 128 components, 8 KiB of text.
    #[test]
    fn rejects_oversize_payloads_counts_and_text() {
        let base = fixture(one_text());
        let mut at = base.clone();
        at.resize(MAX_BYTES, b' ');
        assert!(Surface::parse(&at).is_ok(), "64 KiB");
        let mut over = base.clone();
        over.resize(MAX_BYTES + 1, b' ');
        refused_for(&over, "exceeds 64 KiB");
        refused_for(&vec![b' '; MAX_BYTES + 1], "exceeds 64 KiB");
        let path = std::env::temp_dir().join(format!("agent-surface-oversize-{}.json", std::process::id()));
        std::fs::write(&path, &over).unwrap();
        let read = Surface::read(path.to_str().unwrap());
        std::fs::remove_file(&path).unwrap();
        assert!(read.is_err_and(|e| e.contains("exceeds 64 KiB")), "a file past the cap is refused, not cut");
        let nodes = |n: usize| {
            let kids: Vec<String> = (1..n).map(|i| format!("t{i}")).collect();
            let mut c = vec![json!({"id":"root","component":"Column","children":kids})];
            c.extend((1..n).map(|i| json!({"id":format!("t{i}"),"component":"Text","text":"x"})));
            fixture(Value::Array(c))
        };
        assert!(Surface::parse(&nodes(MAX_NODES)).is_ok(), "128 components");
        refused_for(&nodes(MAX_NODES + 1), "component count");
        let text = |n: usize| fixture(json!([{"id":"root","component":"Text","text":"x".repeat(n)}]));
        assert!(Surface::parse(&text(MAX_TEXT)).is_ok(), "8 KiB of text");
        refused_for(&text(MAX_TEXT + 1), "text exceeds limit");
    }

    /// A tree, not a graph: a cycle through the root or below it, a child
    /// shared, a child missing, a cycle no path from the root reaches.
    #[test]
    fn rejects_cycles_shared_missing_and_unreachable() {
        let col = |id: &str, kids: &[&str]| json!({"id":id,"component":"Column","children":kids});
        let text = |id: &str| json!({"id":id,"component":"Text","text":"x"});
        for (c, why) in [
            (json!([col("root", &["root"])]), "cycle"),
            (json!([col("root", &["a"]), col("a", &["root"])]), "cycle"),
            (json!([col("root", &["a"]), col("a", &["b"]), col("b", &["a"])]), "cycle"),
            (json!([col("root", &["t", "t"]), text("t")]), "shared child"),
            (json!([col("root", &["a", "b"]), col("a", &["t"]), col("b", &["t"]), text("t")]), "shared child"),
            (json!([col("root", &["missing"])]), "missing child"),
            (json!([text("root"), col("a", &["b"]), col("b", &["a"])]), "unreachable component"),
            (json!([text("root"), text("island")]), "unreachable component"),
            (json!([col("a", &["t"]), text("t")]), "requires root"),
            (json!([text("root"), text("root")]), "duplicate component ID"),
        ] {
            refused_for(&fixture(c), why);
        }
    }

    /// JSON nested 20 000 deep, inside the size cap, is refused, not a stack
    /// overflow: in the fields the parser reads and in one it does not know.
    #[test]
    fn rejects_deep_json_nesting() {
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        let n = 20_000;
        let cases = [
            format!(r#"{{"version":"1.0","createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":{}}}}}"#, deep(n)),
            format!(
                r#"{{"version":"1.0","createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":[{{"id":"root","component":"Column","children":{}}}]}}}}"#,
                deep(n)
            ),
            format!(r#"{{"version":"1.0","x":{}}}"#, deep(n)),
            format!(
                r#"{{"version":"1.0","createSurface":{{"surfaceId":"plan","catalogId":"{CATALOG}","components":[{{"id":"root","component":"Text","text":{}}}]}}}}"#,
                deep(n)
            ),
        ];
        for json in cases {
            assert!(json.len() <= MAX_BYTES);
            let e = refused(json.as_bytes());
            assert!(!e.is_empty());
        }
    }

    /// Only A2UI 1.0 on this preview's catalog.
    #[test]
    fn rejects_wrong_catalog_and_version() {
        let at = |version: Value, catalog: Value| {
            serde_json::to_vec(&json!({"version":version,"createSurface":{"surfaceId":"plan","catalogId":catalog,"components":one_text()}}))
                .unwrap()
        };
        assert!(Surface::parse(&at(json!("1.0"), json!(CATALOG))).is_ok());
        for v in ["1.1", "1", "0.9", "1.0 ", "", "v1.0"] {
            refused_for(&at(json!(v), json!(CATALOG)), "unsupported version or catalog");
        }
        for c in
            ["synaps-desk:preview/2", "a2ui:basic", "https://a2ui.org/catalog/basic", "", "SYNAPS-DESK:preview/1", "synaps-desk:preview/1 "]
        {
            refused_for(&at(json!("1.0"), json!(c)), "unsupported version or catalog");
        }
        refused_for(&at(json!(1.0), json!(CATALOG)), "invalid type");
        refused_for(&at(json!("1.0"), json!(null)), "invalid type");
        refused_for(br#"{"createSurface":{}}"#, "missing field");
    }
    #[test]
    fn accepts_native_tree() {
        assert!(
            Surface::parse(&fixture(serde_json::json!([
                {"id":"root","component":"Column","children":["title"]},
                {"id":"title","component":"Text","text":"Plan"}
            ])))
            .is_ok()
        );
    }
    #[test]
    fn rejects_cycles_missing_and_shared_children() {
        for children in [serde_json::json!(["root"]), serde_json::json!(["missing"]), serde_json::json!(["title", "title"])] {
            assert!(
                Surface::parse(&fixture(serde_json::json!([
                    {"id":"root","component":"Column","children":children},
                    {"id":"title","component":"Text","text":"Plan"}
                ])))
                .is_err()
            );
        }
    }
    #[test]
    fn rejects_unknown_capabilities_and_oversized_payloads() {
        assert!(Surface::parse(&fixture(serde_json::json!([{"id":"root","component":"WebView","url":"https://example.com"}]))).is_err());
        assert!(Surface::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }

    /// `--agent-surface` opened its window on a bare app: the palette read a
    /// theme global nothing had set (a panic on the first frame), and the
    /// embedded fonts were never registered. Launched as main launches it,
    /// it draws a frame on the house look.
    #[gpui_kit::test]
    fn the_preview_launches_on_the_house_look(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let surface = Surface::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/agent-ui/plan.json")).expect("the fixture");
        let (theme, raw, _, _) = crate::theme::resolve_theme(None, None, None, None, None);
        let choice = crate::theme::Choice { theme, raw, accent: None, text: crate::theme::DEFAULT_TEXT };
        cx.update(|cx| super::launch(surface, crate::motion::Setting::default(), choice, gpui_kit::WindowOptions::default(), cx));
        cx.run_until_parked();
        let window = cx.update(|cx| cx.windows()[0]);
        cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx)).expect("the window");
        cx.run_until_parked();
        assert!(cx.update(|cx| cx.has_global::<crate::theme::Scheme>()), "the theme");
        assert!(cx.update(|cx| cx.has_global::<crate::fonts::Registered>()), "the house fonts");
        let drawn = gpui_kit::VisualTestContext::from_window(window, cx).debug_bounds("agent-surface");
        assert!(drawn.is_some_and(|b| b.size.width > gpui_kit::px(0.)), "the preview drew");
    }
}
