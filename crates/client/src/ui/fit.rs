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
    /// Where its lines break against the language's rules, if that is the trouble (ク|リック).
    pub broken: Option<String>,
    /// Its glyphs and characters, when they could not be matched up to check its line
    /// breaks: a text left unchecked fails rather than passing unseen.
    pub unmatched: Option<(usize, usize)>,
}

/// How many texts [`find`] has checked the line breaks of, and how many with the language's
/// rules it saw (once per text per look), for the gate to say and to fail on when none were.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct LineBreaksChecked {
    pub checked: usize,
    /// Looks in a language with line rules.
    pub looks: usize,
}

impl LineBreaksChecked {
    /// Whether a look since `before` was in a language with line rules and checked no
    /// text's line breaks.
    pub fn none_since(&self, before: &Self) -> bool {
        self.looks > before.looks && self.checked == before.checked
    }
}

impl std::fmt::Display for Spill {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let sides = ["left", "right", "top", "bottom"];
        let past: Vec<String> = self.past.iter().zip(sides).filter(|(by, _)| **by > 0.5).map(|(by, side)| format!("{by:.0} px {side}")).collect();
        if let Some(other) = &self.over {
            return write!(f, "{:?} runs into {other:?}", self.text);
        }
        if let Some((glyphs, chars)) = self.unmatched {
            return write!(f, "{:?} has {glyphs} glyphs for {chars} characters, so its line breaks went unchecked", self.text);
        }
        if let Some(word) = &self.broken {
            return write!(f, "{:?} breaks a line where its language forbids, at {word:?}", self.text);
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

/// A node's border box as drawn, in logical pixels: scaled as its transform scales it (a
/// banner popping in), about its centre.
fn rect(node: &ComputedNode, transform: &UiGlobalTransform) -> Rect {
    let scale = node.inverse_scale_factor();
    Rect::from_center_size(transform.translation * scale, node.size() * scale * drawn_scale(transform))
}

/// How much a node's transform (its own and its parents') scales it as drawn.
fn drawn_scale(transform: &UiGlobalTransform) -> Vec2 {
    Vec2::new(transform.matrix2.x_axis.length(), transform.matrix2.y_axis.length())
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

/// Whether a text is on screen to be checked: visible, saying something, and laid out (a
/// node in a part of the screen not displayed has no size, and its layout is stale).
fn shown(text: &Text, layout: &TextLayoutInfo, node: &ComputedNode, visible: &InheritedVisibility) -> bool {
    visible.get() && !text.0.trim().is_empty() && layout.size != Vec2::ZERO && node.size().min_element() > 0.0
}

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
        .filter(|(_, text, layout, node, _, visible)| shown(text, layout, node, visible))
        .filter_map(|(entity, text, layout, node, transform, _)| {
            // Layout units to logical pixels, times what the transform scales it by as drawn.
            let scale = node.inverse_scale_factor() * drawn_scale(transform);
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
    // Lines break only where the language lets them (kinsoku: never before a closing
    // bracket or small kana, after an opening bracket, or inside a katakana word). Text is
    // laid out joined (see `crate::lang::Joined`); each glyph is matched to a character of
    // that, with or without the spaces and word joiners, as the layout gives them. A line's
    // end draws nothing.
    let rules = world.get_resource::<crate::lang::Locale>().map(|locale| voxmar_engine::lang::LineRules::of(locale)).unwrap_or_default();
    let mut checked = 0;
    for (_, text, layout, node, _, visible) in texts.iter(world) {
        if rules.is_empty() || text.0.is_ascii() || !shown(text, layout, node, visible) {
            continue;
        }
        // Nothing in it held together: wherever its lines break, they may.
        let Some(joined) = rules.join(&text.0) else {
            checked += 1;
            continue;
        };
        let chars: Vec<char> = joined.chars().collect();
        let joiner = |c: char| c == voxmar_engine::lang::WORD_JOINER;
        // Where the rules hold the text as written together, by each joined character.
        let held = rules.held(&text.0.chars().collect::<Vec<_>>());
        let written: Vec<usize> = chars.iter().scan(0, |n, &c| Some(std::mem::replace(n, *n + usize::from(!joiner(c))))).collect();
        // Every character; spaces drawn but joiners not; neither; joiners drawn but spaces not.
        // Spaces kept comes first, so one joiner per space can't match the wrong way round.
        let mappings: [fn(char) -> bool; 4] = [
            |_| true,
            |c| c != voxmar_engine::lang::WORD_JOINER,
            |c| !c.is_whitespace() && c != voxmar_engine::lang::WORD_JOINER,
            |c| !c.is_whitespace(),
        ];
        let drawn = |c: char| !matches!(c, '\n' | '\r');
        let Some(at) = mappings.iter().map(|keep| (0..chars.len()).filter(|&i| drawn(chars[i]) && keep(chars[i])).collect::<Vec<usize>>()).find(|at| at.len() == layout.glyphs.len()) else {
            found.push(Spill { text: text.0.clone(), past: [0.0; 4], own: true, size: Vec2::ZERO, lines: layout.size, over: None, broken: None, unmatched: Some((layout.glyphs.len(), chars.len())) });
            continue;
        };
        checked += 1;
        let first_written = |i: usize| (i..chars.len()).find(|&i| !joiner(chars[i])).map(|i| written[i]);
        if let Some(at) = (1..at.len()).filter(|&k| layout.glyphs[k].line_index != layout.glyphs[k - 1].line_index).find_map(|k| first_written(at[k]).filter(|&at| held[at])) {
            let plain: Vec<char> = text.0.chars().collect();
            let break_at = format!("{}|{}", plain[at.saturating_sub(4)..at].iter().collect::<String>(), plain[at..(at + 4).min(plain.len())].iter().collect::<String>());
            found.push(Spill { text: text.0.clone(), past: [0.0; 4], own: true, size: Vec2::ZERO, lines: layout.size, over: None, broken: Some(break_at), unmatched: None });
        }
    }
    if !rules.is_empty() {
        let mut tally = world.get_resource_or_init::<LineBreaksChecked>();
        tally.checked += checked;
        tally.looks += 1;
    }
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
                found.push(Spill { text, past: [0.0; 4], own: false, size: second.size(), lines: first.size(), over: Some(candidates[j].1.clone()), broken: None, unmatched: None });
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
                found.push(Spill { text: candidates[i].1.clone(), past: by, own: false, size: screen.size(), lines: seen.size(), over: None, broken: None, unmatched: None });
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
    let icons: Vec<(Entity, Rect)> = drawn.into_iter().filter_map(|(entity, icon)| Some((entity, clipped(world, &mut parents, &mut boxes, entity, icon)?))).collect();
    // A text drawn on a layer above an icon's (a tooltip, on its own backing over the
    // slots) covers it rather than runs into it.
    let layer = |mut entity: Entity| loop {
        if let Some(z) = world.get::<GlobalZIndex>(entity) {
            return z.0;
        }
        match world.get::<ChildOf>(entity) {
            Some(parent) => entity = parent.parent(),
            None => return 0,
        }
    };
    for &(i, seen) in &shown {
        let text = &candidates[i].1;
        if text.chars().filter(|c| !c.is_whitespace()).count() <= 2 {
            continue;
        }
        let above = layer(candidates[i].0);
        if let Some((_, icon)) = icons.iter().filter(|(entity, _)| layer(*entity) >= above).find(|(_, icon)| {
            let both = seen.intersect(*icon);
            both.width() > slack && both.height() > slack
        }) {
            found.push(Spill { text: text.clone(), past: [0.0; 4], own: false, size: icon.size(), lines: seen.size(), over: Some("an icon".into()), broken: None, unmatched: None });
        }
    }
    for (entity, text, lines, own) in candidates {

        // Across, a text past its own box runs into its neighbours. Up and down, a glyph may
        // stand a little proud of a one-line box (`TightLine`): the element around it judges.
        // Kanji drawn on a finer grid than ours (three quarters, `fallback.ron`) land between
        // screen pixels, and their smoothed edge can show a pixel either side of the box.
        let by = past(lines, own);
        if spills(&by, false, slack + units_per_pixel) {
            found.push(Spill { text, past: by, own: true, size: own.size(), lines: lines.size(), over: None, broken: None, unmatched: None });
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
                found.push(Spill { text, past: by, own: false, size: outer.size(), lines: lines.size(), over: None, broken: None, unmatched: None });
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box is measured as drawn: a banner shrunk to pop in is smaller on screen, about its
    /// centre, than its layout.
    #[test]
    fn boxes_are_measured_as_drawn() {
        let node = ComputedNode { size: Vec2::new(100.0, 40.0), inverse_scale_factor: 1.0, ..Default::default() };
        let plain = UiGlobalTransform::from(bevy::math::Affine2::from_translation(Vec2::new(200.0, 100.0)));
        assert_eq!(rect(&node, &plain), Rect::from_center_size(Vec2::new(200.0, 100.0), Vec2::new(100.0, 40.0)));
        let shrunk = UiGlobalTransform::from(bevy::math::Affine2::from_scale_angle_translation(Vec2::splat(0.5), 0.0, Vec2::new(200.0, 100.0)));
        assert_eq!(rect(&node, &shrunk), Rect::from_center_size(Vec2::new(200.0, 100.0), Vec2::new(50.0, 20.0)));
    }

    /// What [`find`] says of one Japanese text laid out with its glyphs on `lines`, in a node
    /// of `size` with a layout of `laid_out`, and how many texts' line breaks it checked.
    fn look_at_node(text: &str, lines: &[usize], size: Vec2, laid_out: Vec2, visibility: InheritedVisibility) -> (Vec<String>, usize) {
        use bevy::text::{GlyphAtlasInfo, PositionedGlyph};
        let content = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content");
        let mut world = World::new();
        world.insert_resource(crate::lang::Locale::load(&content, "ja"));
        let atlas_info = GlyphAtlasInfo { texture: AssetId::default(), rect: Rect::default(), offset: Vec2::ZERO, is_alpha_mask: false };
        let glyphs = lines.iter().map(|&line_index| PositionedGlyph { position: Vec2::ZERO, atlas_info: atlas_info.clone(), section_index: 0, line_index }).collect();
        let layout = TextLayoutInfo { glyphs, size: laid_out, ..Default::default() };
        world.spawn((Text::new(text), layout, ComputedNode { size, ..default() }, UiGlobalTransform::default(), visibility));
        let found: Vec<String> = find(&mut world).iter().map(ToString::to_string).collect();
        (found, world.resource::<LineBreaksChecked>().checked)
    }

    /// A Japanese line broken inside a katakana word is found however the layout gives its
    /// glyphs (with word joiners or without), and a text whose glyphs can't be matched to
    /// its characters fails rather than passing unchecked.
    #[test]
    fn line_breaks_against_the_rules_are_found() {
        let look_at = |text: &str, lines: &[usize]| look_at_node(text, lines, Vec2::new(100.0, 20.0), Vec2::splat(10.0), InheritedVisibility::VISIBLE);
        let look = |lines: &[usize]| look_at("クリックする", lines);
        // Laid out as ク⁠リ⁠ッ⁠クする, a glyph each with the joiners (9) or without (6).
        let (found, checked) = look(&[0, 0, 1, 1, 1, 1, 1, 1, 1]);
        assert!(checked == 1 && found.len() == 1 && found[0].contains("ク|リック"), "{found:?}");
        let (found, checked) = look(&[0, 1, 1, 1, 1, 1]);
        assert!(checked == 1 && found.len() == 1 && found[0].contains("ク|リック"), "{found:?}");
        // Broken where it may break: after the word.
        assert_eq!(look(&[0, 0, 0, 0, 1, 1]), (vec![], 1));
        // ボタン を押す joined is ボ⁠タ⁠ン を押す: 2 joiners, 1 space. Drawn with the space and
        // without the joiners (7 glyphs), a break inside the word is still found where it is.
        let (found, checked) = look_at("ボタン を押す", &[0, 1, 1, 1, 1, 1, 1]);
        assert!(checked == 1 && found.len() == 1 && found[0].contains("ボ|タン を"), "{found:?}");
        assert_eq!(look_at("ボタン を押す", &[0, 0, 0, 0, 1, 1, 1]), (vec![], 1));
        // One joiner and one space (ア⁠イ う): dropping either leaves 4, and the space kept is
        // tried first, so the glyphs line up with the characters they are.
        let (found, checked) = look_at("アイ う", &[0, 1, 1, 1]);
        assert!(checked == 1 && found.len() == 1 && found[0].contains("ア|イ う"), "{found:?}");
        assert_eq!(look_at("アイ う", &[0, 0, 0, 1]), (vec![], 1));
        // A break at the space, misread as one inside アイ were the joiner taken as drawn.
        assert_eq!(look_at("アイ う", &[0, 0, 1, 1]), (vec![], 1));
        // Lines of their own: ボ⁠タ⁠ン を押す\nク⁠リ⁠ッ⁠クする is drawn with no glyph for the line's
        // end (13 glyphs, spaces kept), and breaks only where it may, or inside a word.
        let two_lines = "ボタン を押す\nクリックする";
        assert_eq!(look_at(two_lines, &[0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1]), (vec![], 1));
        let (found, checked) = look_at(two_lines, &[0, 0, 0, 0, 0, 0, 0, 1, 2, 2, 2, 2, 2]);
        assert!(checked == 1 && found.len() == 1 && found[0].contains("ク|リック"), "{found:?}");
        let (found, checked) = look(&[0, 0, 0]);
        assert!(checked == 0 && found.len() == 1 && found[0].contains("unchecked"), "{found:?}");
    }

    /// Only a text on screen has its line breaks checked, as only one is checked for fitting:
    /// one hidden, or in a node or layout not laid out (its glyphs stale or none), is passed
    /// over rather than failed as unchecked or judged by an old layout; laid out, the same
    /// text's bad break is still found.
    #[test]
    fn line_breaks_are_checked_only_on_screen() {
        let bad = [0, 0, 1, 1, 1, 1, 1, 1, 1];
        let (size, laid_out) = (Vec2::new(100.0, 20.0), Vec2::splat(10.0));
        let (found, checked) = look_at_node("クリックする", &bad, size, laid_out, InheritedVisibility::VISIBLE);
        assert!(checked == 1 && found.len() == 1 && found[0].contains("ク|リック"), "{found:?}");
        assert_eq!(look_at_node("クリックする", &bad, size, laid_out, InheritedVisibility::HIDDEN), (vec![], 0));
        assert_eq!(look_at_node("クリックする", &bad, Vec2::ZERO, laid_out, InheritedVisibility::VISIBLE), (vec![], 0));
        assert_eq!(look_at_node("クリックする", &bad, Vec2::new(100.0, 0.0), laid_out, InheritedVisibility::VISIBLE), (vec![], 0));
        assert_eq!(look_at_node("クリックする", &bad, size, Vec2::ZERO, InheritedVisibility::VISIBLE), (vec![], 0));
        // Nothing in it held together (kanji and hiragana only): checked, and fine however
        // it breaks.
        assert_eq!(look_at_node("木を切る", &[0, 1, 1, 2], size, laid_out, InheritedVisibility::VISIBLE), (vec![], 1));
        // Not laid out yet, with no glyphs: passed over, not failed as unchecked.
        assert_eq!(look_at_node("クリックする", &[], Vec2::ZERO, Vec2::ZERO, InheritedVisibility::VISIBLE), (vec![], 0));
    }

    /// A look in a language with line rules that checks nothing is told apart from one that
    /// checks something and from one in a language without rules.
    #[test]
    fn a_look_that_checks_no_line_break_is_told() {
        let before = LineBreaksChecked { checked: 5, looks: 2 };
        assert!(LineBreaksChecked { checked: 5, looks: 3 }.none_since(&before));
        assert!(!LineBreaksChecked { checked: 7, looks: 3 }.none_since(&before));
        assert!(!before.none_since(&before));
    }

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
