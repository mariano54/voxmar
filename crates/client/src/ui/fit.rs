//! Text that does not fit: every visible text whose glyphs reach past its own node, or
//! come within a pixel of the inside of an element it sits in (a panel, button, chip,
//! meter, slot: anything drawn), or past anything that clips. The gate walks every screen in every
//! language with it, so a longer translation or a taller font shows up as a failure
//! rather than as words spilling out of a button.

use bevy::prelude::*;
use bevy::text::TextLayoutInfo;
use bevy::ui::UiGlobalTransform;

/// Text may reach a pixel past its box (of the layout, or more at a fractional interface
/// size, where positions round to the screen's pixels) before it counts.
const SLACK: f32 = 1.0;
/// The width of a drawn element's frame (one pixel of its art), and the room words keep
/// inside it on every side (one more): words that touch a frame count as not fitting.
const FRAME: f32 = crate::ui::skin::PX;
const ROOM: f32 = crate::ui::skin::PX;

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
    /// Another text it runs into, if that is the trouble.
    pub over: Option<String>,
}

impl std::fmt::Display for Spill {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let sides = ["left", "right", "top", "bottom"];
        let past: Vec<String> = self.past.iter().zip(sides).filter(|(by, _)| **by > 0.5).map(|(by, side)| format!("{by:.0} px {side}")).collect();
        if let Some(other) = &self.over {
            return write!(f, "{:?} runs into {other:?}", self.text);
        }
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
fn spills(past: &[f32; 4], vertical: bool, slack: f32) -> bool {
    past[..2].iter().any(|&by| by > slack) || (vertical && past[2..].iter().any(|&by| by > slack))
}

/// A node's border box, in logical pixels.
fn rect(node: &ComputedNode, transform: &UiGlobalTransform) -> Rect {
    let scale = node.inverse_scale_factor();
    Rect::from_center_size(transform.translation * scale, node.size() * scale)
}

type Boxes<'a> = (&'a ComputedNode, &'a UiGlobalTransform, &'a Node, Has<ImageNode>, Option<&'a BackgroundColor>, &'a InheritedVisibility);

/// As much of `area` (of `entity`) as shows past every clipping element around it (a
/// scrolling list hides what lies beyond its edges), if any.
fn clipped(world: &World, parents: &mut QueryState<&ChildOf>, boxes: &mut QueryState<Boxes>, entity: Entity, area: Rect) -> Option<Rect> {
    let mut seen = area;
    let mut at = entity;
    while let Ok(parent) = parents.get(world, at).map(ChildOf::parent) {
        at = parent;
        let Ok((node, transform, style, ..)) = boxes.get(world, at) else { break };
        if style.overflow.x != OverflowAxis::Visible || style.overflow.y != OverflowAxis::Visible {
            seen = seen.intersect(rect(node, transform));
        }
    }
    (seen.width() > 0.0 && seen.height() > 0.0).then_some(seen)
}

/// A text drawn only to set off another (the ring round a stack count), which stands where
/// that one does: the check leaves it out.
#[derive(Component)]
pub struct Decoration;

/// Every visible text that does not fit, each once (against the first box it spills from,
/// its own node first).
pub fn find(world: &mut World) -> Vec<Spill> {
    // A layout unit is this many of the screen's pixels: below one, rounding to whole
    // pixels can move an edge by more than a unit.
    let units_per_pixel = world.query::<&ComputedNode>().iter(world).map(ComputedNode::inverse_scale_factor).fold(1.0f32, f32::max);
    let slack = (SLACK * units_per_pixel).max(SLACK);
    let mut texts = world.query_filtered::<(Entity, &Text, &TextLayoutInfo, &ComputedNode, &UiGlobalTransform, &InheritedVisibility), Without<Decoration>>();
    let mut boxes = world.query::<Boxes>();
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
    // Texts running into one another, as far as each shows (a scrolling list hides what
    // lies past its edges).
    let shown: Vec<(usize, Rect)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(i, (entity, _, lines, _))| Some((i, clipped(world, &mut parents, &mut boxes, *entity, *lines)?)))
        .collect();
    for (a, &(i, first)) in shown.iter().enumerate() {
        for &(j, second) in &shown[a + 1..] {
            let both = first.intersect(second);
            if both.width() > slack && both.height() > slack {
                let text = candidates[i].1.clone();
                found.push(Spill { text, past: [0.0; 4], own: false, size: second.size(), lines: first.size(), over: Some(candidates[j].1.clone()) });
            }
        }
    }
    // Nothing may reach the window's edges: the screen is the widest of the roots.
    let mut roots = world.query_filtered::<(&ComputedNode, &UiGlobalTransform), (With<Node>, Without<ChildOf>)>();
    let screen = roots.iter(world).map(|(node, transform)| rect(node, transform)).reduce(|a, b| a.union(b));
    if let Some(screen) = screen {
        for &(i, seen) in &shown {
            let by = past(seen, screen.inflate(-ROOM));
            if spills(&by, true, slack) {
                found.push(Spill { text: candidates[i].1.clone(), past: by, own: false, size: screen.size(), lines: seen.size(), over: None });
            }
        }
    }
    // Up to two characters (a stack's count) may sit on an icon's corner, as on every
    // slot; anything longer, such as a range of amounts, must not cover one.
    // Icons are the items' and skills' pictures (not frames, bars, covers or logos).
    let pictures: std::collections::HashSet<AssetId<Image>> = world
        .get_resource::<crate::hud::HudImages>()
        .map(|images| images.icons.iter().chain(&images.skills).map(Handle::id).collect())
        .unwrap_or_default();
    let mut images = world.query::<(Entity, &ImageNode, &ComputedNode, &UiGlobalTransform, &InheritedVisibility)>();
    let drawn: Vec<(Entity, Rect)> = images
        .iter(world)
        .filter(|(_, image, node, _, visible)| visible.get() && pictures.contains(&image.image.id()) && node.size().min_element() > 0.0)
        .map(|(entity, _, node, transform, _)| (entity, rect(node, transform)))
        .collect();
    let icons: Vec<Rect> = drawn.into_iter().filter_map(|(entity, icon)| clipped(world, &mut parents, &mut boxes, entity, icon)).collect();
    for &(i, seen) in &shown {
        let text = &candidates[i].1;
        if text.chars().filter(|c| !c.is_whitespace()).count() <= 2 {
            continue;
        }
        if let Some(icon) = icons.iter().find(|icon| {
            let both = seen.intersect(**icon);
            both.width() > slack && both.height() > slack
        }) {
            found.push(Spill { text: text.clone(), past: [0.0; 4], own: false, size: icon.size(), lines: seen.size(), over: Some("an icon".into()) });
        }
    }
    for (entity, text, lines, own) in candidates {

        // Across, a text past its own box runs into its neighbours. Up and down, a glyph may
        // stand a little proud of a one-line box (`TightLine`): the element around it judges.
        // Kanji drawn on a finer grid than ours (three quarters, `fallback.ron`) land between
        // screen pixels, and their smoothed edge can show a pixel either side of the box.
        let by = past(lines, own);
        if spills(&by, false, slack + units_per_pixel) {
            found.push(Spill { text, past: by, own: true, size: own.size(), lines: lines.size(), over: None });
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
            // A drawn element's frame is one of its pixels wide: words belong inside it, with
            // room to spare.
            let outer = rect(node, transform);
            let inner = if drawn { outer.inflate(-(FRAME + ROOM)) } else { outer };
            let by = past(lines, inner);
            if node.size().min_element() > 0.0 && spills(&by, vertical, slack) {
                found.push(Spill { text, past: by, own: false, size: outer.size(), lines: lines.size(), over: None });
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
        assert!(!spills(&[0.5, 0.0, 0.0, 0.0], true, SLACK));
        assert!(spills(&[0.0, 0.0, 0.0, 6.0], true, SLACK));
        assert!(!spills(&[0.0, 0.0, 0.0, 6.0], false, SLACK));
    }
}
