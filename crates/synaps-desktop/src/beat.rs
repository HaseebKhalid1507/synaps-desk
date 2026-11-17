//! The window's content: the app's view, cached, and the beat layer over it
//! (motion.md §10 amendment 13).
//!
//! A beat changes a few pixels: a running glyph's 1 px scan row, a
//! breathing cell. Redrawing the app's view for it (its render, layout,
//! prepaint and paint, ~2.5 ms) is most of what a beat cost. So the app's
//! view is a cached view: GPUI replays its last frame until something it
//! drew from changes (it is notified: an event, a hover, a scroll, a
//! focus, a resize, a refresh). The glyphs that change on the beat alone
//! are drawn still and record where they were painted
//! ([`crate::motion::Site`]); a beat wakes only the [`BeatLayer`], which
//! paints their rows over the replayed frame and arms the next beat.
//!
//! Painted in the root's tree, the layer lies under every deferred element
//! (menus, the picker, the gear menu), tooltips and drags, as the glyphs do.

use std::rc::Rc;

use gpui_kit::*;

use crate::app::AppView;
use crate::motion::Motion;

/// The window root's view: the app, cached, and the beat layer.
pub(crate) struct Shell {
    app: Entity<AppView>,
    beat: Entity<BeatLayer>,
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(self.app.clone().cached(StyleRefinement::default().size_full())).child(self.beat.clone())
    }
}

/// Paints what changes on the beat alone, over the app's cached frame.
pub(crate) struct BeatLayer {
    fx: Rc<Motion>,
    app: EntityId,
}

impl Render for BeatLayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (fx, app, me) = (self.fx.clone(), self.app, cx.entity_id());
        canvas(|_, _, _| {}, move |_, _, window, cx| fx.paint_beats(window, cx, app, me)).absolute().inset_0()
    }
}

/// The beat goes to the layer, unless `SYNAPS_DESKTOP_BEAT_IN_FRAME=1`
/// (measuring): then every beat is the app's frame, as before the layer.
/// The app reads it; tests open their windows either way explicitly.
pub(crate) fn layered_by_env() -> bool {
    std::env::var_os("SYNAPS_DESKTOP_BEAT_IN_FRAME").is_none_or(|v| v != "1")
}

/// Opens the app's window: [`gpui_kit::open_window`] with the app's view
/// under a [`Shell`], its beat on the layer (`layered`) or in its frame.
/// Returns the app's view.
pub(crate) fn open_window(
    options: WindowOptions,
    cx: &mut App,
    layered: bool,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<AppView>,
) -> anyhow::Result<(AnyWindowHandle, Entity<AppView>)> {
    let mut built = None;
    let (window, _) = gpui_kit::open_window(options, cx, |window, cx| {
        let app = build(window, cx);
        built = Some(app.clone());
        let fx = app.read(cx).fx.clone();
        if layered {
            fx.set_layer();
        }
        let beat = cx.new(|_| BeatLayer { fx, app: app.entity_id() });
        cx.new(|_| Shell { app, beat })
    })?;
    Ok((window, built.expect("open_window ran its build closure")))
}
