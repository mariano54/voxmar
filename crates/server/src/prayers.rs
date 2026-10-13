//! Prayers on the server (`voxmar_engine::prayer`, the design wiki's Combat feel tab): a
//! player turns them on and off, the points they draw drain by the tick, and at none left
//! every prayer goes out. An altar fills the points and takes offerings for Magic
//! experience; out of a fight with nothing on, points come back slowly. What they do in a
//! fight is read where blows land (`creatures`), how fast health comes back in `process`,
//! and what death keeps in `death`.

use voxmar_engine::lang::Words;
use voxmar_engine::prayer::PrayerId;
use voxmar_engine::traits;
use voxmar_protocol::{ChatKind, PlayerId, ServerMessage, TICKS_PER_SECOND};

use crate::Server;

impl Server {
    /// The most prayer points `player` has: by their Magic level.
    pub(crate) fn prayer_most(&self, player: PlayerId) -> f32 {
        let book = &self.content.prayers;
        book.most(self.level(player, book.skill))
    }

    /// A player turned a prayer on or off. On needs the Magic level it opens at and a point
    /// left; it puts out any prayer on that does the same kind of thing.
    pub(crate) fn pray(&mut self, id: PlayerId, prayer: PrayerId, on: bool) {
        let book = self.content.prayers.clone();
        let Some(def) = book.get(prayer) else { return };
        let level = self.level(id, book.skill);
        let Some(player) = self.players.get_mut(&id) else { return };
        // The dead keep their prayers on until death has counted what they keep (Keepsake).
        if !player.health.alive() {
            return;
        }
        if !on {
            player.praying.on.retain(|&p| p != prayer);
        } else if level >= def.level && player.praying.points > 0.0 {
            player.praying.on = book.turn_on(&player.praying.on, prayer);
        }
    }

    /// A tick of prayer: the prayers on drain their points, and none left puts them out; with
    /// none on and no blow either way for the rules' `calm`, a point comes back every
    /// `regain` seconds. Each player hears of their points and prayers when they change.
    pub(crate) fn tick_prayers(&mut self) {
        let book = self.content.prayers.clone();
        let seconds = 1.0 / TICKS_PER_SECOND as f32;
        let calm = (book.rules.calm * TICKS_PER_SECOND as f32).ceil() as u64;
        let tick = self.tick;
        let ids: Vec<PlayerId> = self.players.keys().copied().collect();
        for id in ids {
            let most = self.prayer_most(id);
            let player = self.players.get_mut(&id).expect("listed");
            if !player.joined {
                continue;
            }
            // The dead keep theirs on until death has counted what they keep (`death`), and
            // neither drain nor regain.
            let alive = player.health.alive();
            let praying = &mut player.praying;
            if alive && praying.on.is_empty() {
                if tick >= player.last_blow + calm && praying.points < most {
                    praying.regain_clock += seconds;
                    if praying.regain_clock >= book.rules.regain {
                        praying.regain_clock -= book.rules.regain;
                        praying.points = (praying.points + 1.0).min(most);
                    }
                } else {
                    praying.regain_clock = 0.0;
                }
            } else if alive {
                praying.regain_clock = 0.0;
                praying.points -= book.drain(&praying.on) * seconds;
                if praying.points <= 0.0 {
                    praying.points = 0.0;
                    praying.on.clear();
                }
            }
            praying.points = praying.points.min(most);
            // A point part drained still shows, so a prayer never goes out at 0 shown.
            let shown = (praying.points.ceil() as u16, most.round() as u16, praying.on.clone());
            if player.prayers_sent.as_ref() != Some(&shown) {
                let (points, most, on) = shown.clone();
                player.connection.send(ServerMessage::Prayers { points, most, on });
                player.prayers_sent = Some(shown);
            }
        }
    }

    /// A player prays at an altar (`base:pray`): their points come back in full, and an
    /// offering in hand (a bone) is taken for Magic experience, the rules' `offer_xp` for
    /// each bone it is worth.
    pub(crate) fn pray_at_altar(&mut self, id: PlayerId) {
        let book = self.content.prayers.clone();
        let Some(player) = self.players.get_mut(&id) else { return };
        if let (Some(worth), Some(skill)) = (player.held().and_then(|item| book.offering(item)), book.skill) {
            let selected = player.selected;
            if player.inventory.take_one(selected).is_some() {
                let tenths = (10.0 * book.rules.offer_xp * worth).round() as u32;
                player.skills.grant(skill, tenths);
                player.changed(traits::INVENTORY);
                player.changed(traits::SKILLS);
                player.connection.send(ServerMessage::Xp { skill, tenths });
            }
        }
        // Filled to what the offering may just have raised.
        let most = self.prayer_most(id);
        let player = self.players.get_mut(&id).expect("found above");
        if player.praying.points < most {
            player.praying.points = most;
            player.praying.regain_clock = 0.0;
            player.connection.send(ServerMessage::Chat { kind: ChatKind::Info, text: Words::key("prayer.restored") });
        }
    }
}
