//! Opt-in, data-only A2UI 1.0 preview. No daemon, URLs, code, or permissions.
use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::*;
use serde::Deserialize;

use crate::fonts::TextPx;

pub(crate) const CATALOG: &str = "synaps-desk:preview/1";
const MAX_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Message {
    version: String,
    #[serde(rename = "createSurface")]
    create: Surface,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Surface {
    pub surface_id: String,
    catalog_id: String,
    pub components: Vec<Component>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Component {
    pub id: String,
    #[serde(flatten)]
    pub kind: Kind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "component", deny_unknown_fields)]
pub(crate) enum Kind {
    Text { text: String },
    Column { children: Vec<String> },
    Row { children: Vec<String> },
}

impl Surface {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("surface exceeds 64 KiB".into());
        }
        let message: Message = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let surface = message.create;
        if message.version != "1.0" || surface.catalog_id != CATALOG {
            return Err("unsupported version or catalog".into());
        }
        if surface.surface_id.is_empty() || surface.surface_id.len() > 128 || surface.components.len() > 128 {
            return Err("invalid surface identity or component count".into());
        }
        let mut ids = BTreeMap::new();
        for component in &surface.components {
            if component.id.is_empty() || component.id.len() > 128 || ids.insert(component.id.as_str(), component).is_some() {
                return Err("invalid or duplicate component ID".into());
            }
        }
        if !ids.contains_key("root") {
            return Err("preview requires root".into());
        }
        // Require a tree, not a DAG whose repeated references amplify rendering.
        let mut seen = BTreeSet::new();
        fn visit<'a>(id: &'a str, ids: &BTreeMap<&str, &'a Component>, seen: &mut BTreeSet<&'a str>, depth: usize) -> Result<(), String> {
            if depth > 16 || !seen.insert(id) {
                return Err("cycle, shared child, or depth limit".into());
            }
            let node = ids.get(id).ok_or("missing child")?;
            match &node.kind {
                Kind::Text { text } if text.len() > 8192 => return Err("text exceeds limit".into()),
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
    use super::{CATALOG, MAX_BYTES, Surface};
    fn fixture(components: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({"version":"1.0", "createSurface":{
            "surfaceId":"plan", "catalogId": CATALOG, "components":components
        }}))
        .unwrap()
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
