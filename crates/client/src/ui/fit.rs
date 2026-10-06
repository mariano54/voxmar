//! Text that does not fit: every visible text whose glyphs reach past its own node
//! or past an element it sits in (a panel, button, chip, meter, slot: anything drawn, or
//! anything that clips), by more than a pixel. The gate walks every screen in every
//! language with it, so a longer translation or a taller font shows up as a failure
//! rather than as words spilling out of a button.

use bevy::prelude::*;
use bevy::text::TextLayoutInfo;
use bevy::ui::UiGlobalTransform;

/// Pixels (of the 1920-wide layout) text may reach past its box before it counts.
const SLACK: f32 = 1.0;
/// The width of a drawn element's frame: one pixel of its art.
const FRAME: f32 = crate::ui::skin::PX;

/// A text reaching past a box: what it says, and how far past it reaches on each side
/// (left, right, top, bottom), in logical pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Spill {
    pub text: String,
    pub past: [f32; 4],
    /// What the box is: the text's own node, or an element around it, and its size.
    pub own: bool,
    pub size: Vec2,
    /// The text's lines' size.
    pub lines: Vec2,
}

impl std::fmt::Display for Spill {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let sides = ["left", "right", "top", "bottom"];
        let past: Vec<String> = self.past.iter().zip(sides).filter(|(by, _)| **by > SLACK).map(|(by, side)| format!("{by:.0} px {side}")).collect();
        let what = if self.own { "its own box" } else { "the element it sits in" };
        let (size, lines) = (self.size, self.lines);
        write!(f, "{:?} ({:.0}×{:.0}) reaches past {what} ({:.0}×{:.0}) by {}", self.text, lines.x, lines.y, size.x, size.y, past.join(", "))
    }
}

/// How far `inner` reaches past `outer` on each side (left, right, top, bottom).
pub fn past(inner: Rect, outer: Rect) -> [f32; 4] {
    [outer.min.x - inner.min.x, inner.max.x - outer.max.x, outer.min.y - inner.min.y, inner.max.y - outer.max.y].map(|by| by.max(0.0))
}

/// Whether any side reaches past by more than the slack; `vertical` false ignores the
/// top and bottom (a scrolling list may cut lines off there; that is what scrolling is).
fn spills(past: &[f32; 4], vertical: bool) -> bool {
    past[..2].iter().any(|&by| by > SLACK) || (vertical && past[2..].iter().any(|&by| by > SLACK))
}

/// A node's border box, in logical pixels.
fn rect(node: &ComputedNode, transform: &UiGlobalTransform) -> Rect {
    let scale = node.inverse_scale_factor();
    Rect::from_center_size(transform.translation * scale, node.size() * scale)
}

/// Every visible text that does not fit, each once (against the first box it spills from,
/// its own node first).
pub fn find(world: &mut World) -> Vec<Spill> {
    let mut texts = world.query::<(Entity, &Text, &TextLayoutInfo, &ComputedNode, &UiGlobalTransform, &InheritedVisibility)>();
    let mut boxes = world.query::<(&ComputedNode, &UiGlobalTransform, &Node, Has<ImageNode>, Option<&BackgroundColor>, &InheritedVisibility)>();
    let mut parents = world.query::<&ChildOf>();
    let mut found = Vec::new();
    let candidates: Vec<(Entity, String, Rect, Rect)> = texts
        .iter(world)
        // A node not laid out (in a part of the screen not displayed) has no size.
        .filter(|(_, text, layout, node, _, visible)| visible.get() && !text.0.trim().is_empty() && layout.size != Vec2::ZERO && node.size().min_element() > 0.0)
        .filter_map(|(entity, text, layout, node, transform, _)| {
            let scale = node.inverse_scale_factor();
            let own = rect(node, transform);
            let inset = Vec2::new(node.border.min_inset.x + node.padding.min_inset.x, node.border.min_inset.y + node.padding.min_inset.y) * scale;
            // Its ink: the glyphs as drawn, so a space left at the end of a wrapped line
            // (which the layout's size counts) is not taken for spilling.
            let ink = layout
                .glyphs
                .iter()
                .filter(|glyph| glyph.atlas_info.rect.size().min_element() > 0.0)
                .map(|glyph| Rect::from_center_size(glyph.position, glyph.atlas_info.rect.size()))
                .reduce(|a, b| a.union(b))?;
            let min = own.min + inset;
            Some((entity, text.0.clone(), Rect { min: min + ink.min * scale, max: min + ink.max * scale }, own))
        })
        .collect();
    for (entity, text, lines, own) in candidates {

        // Across, a text past its own box runs into its neighbours. Up and down, a glyph may
        // stand a little proud of a one-line box (`TightLine`): the element around it judges.
        let by = past(lines, own);
        if spills(&by, false) {
            found.push(Spill { text, past: by, own: true, size: own.size(), lines: lines.size() });
            continue;
        }
        // Past a scrolling list, lines may be cut off above and below by every box out to
        // the screen: that is the scrolling.
        let mut vertical = true;
        let mut at = entity;
        while let Ok(parent) = parents.get(world, at).map(ChildOf::parent) {
            at = parent;
            let Ok((node, transform, style, image, background, visible)) = boxes.get(world, at) else { break };
            let drawn = image || background.is_some_and(|b| b.0.alpha() > 0.0);
            let clips = style.overflow.x != OverflowAxis::Visible || style.overflow.y != OverflowAxis::Visible;
            if !visible.get() || !(drawn || clips) {
                continue;
            }
            vertical &= style.overflow.y != OverflowAxis::Scroll;
            // A drawn element's frame is one of its pixels wide: words belong inside it.
            let outer = rect(node, transform);
            let inner = if image { outer.inflate(-FRAME) } else { outer };
            let by = past(lines, inner);
            if node.size().min_element() > 0.0 && spills(&by, vertical) {
                found.push(Spill { text, past: by, own: false, size: outer.size(), lines: lines.size() });
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reaching_past_counts_each_side() {
        let outer = Rect::new(0.0, 0.0, 100.0, 20.0);
        assert_eq!(past(Rect::new(10.0, 2.0, 90.0, 18.0), outer), [0.0; 4]);
        assert_eq!(past(Rect::new(-4.0, 2.0, 108.0, 26.0), outer), [4.0, 8.0, 0.0, 6.0]);
        assert!(!spills(&[0.5, 0.0, 0.0, 0.0], true));
        assert!(spills(&[0.0, 0.0, 0.0, 6.0], true));
        assert!(!spills(&[0.0, 0.0, 0.0, 6.0], false));
    }
}
