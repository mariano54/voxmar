//! The screenshot key (F2 unless rebound): the frame as shown, HUD and all, copied to the
//! clipboard as an image and saved as a PNG in the screenshots folder beside the settings.
//!
//! The frame is read back the way the gate and captures take theirs (Bevy's `Screenshot`,
//! answered a few frames later); the encoding, the file and the clipboard are handled on a
//! worker thread, so a screenshot never stalls a frame. A line in the chat says how it went.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::Mutex;

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::tasks::AsyncComputeTaskPool;
use voxmar_protocol::ChatKind;

use crate::hud::console::Console;
use crate::lang::Locale;
use crate::menu::Screen;
use crate::player::UiState;
use crate::settings::Settings;

pub struct ScreenshotPlugin;

impl Plugin for ScreenshotPlugin {
    fn build(&self, app: &mut App) {
        let (sender, receiver) = channel();
        app.insert_resource(Shots { sender, receiver: Mutex::new(receiver), taken: 0 })
            .init_resource::<LastShot>()
            .add_systems(Update, (take, report).run_if(in_state(Screen::InGame)));
    }
}

/// How a screenshot went, from the worker that kept it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kept {
    /// Saved here, and copied to the clipboard (true), not asked to be (false), or not
    /// copied, with why.
    Saved { path: PathBuf, copied: Result<bool, String> },
    Failed(String),
}

/// The channel workers answer on, and how many shots have been asked for.
#[derive(Resource)]
pub struct Shots {
    sender: Sender<Kept>,
    receiver: Mutex<Receiver<Kept>>,
    pub taken: u32,
}

/// The folder screenshots go in: `screenshots` in the game's settings folder.
pub fn folder() -> Option<PathBuf> {
    crate::settings::config_dir().map(|dir| dir.join("screenshots"))
}

/// Where a game drawing off screen (`--gate`, `--capture`) draws, to shoot that instead
/// of a window.
#[derive(Resource, Clone)]
pub struct Offscreen(pub Handle<Image>);

/// A folder to save in instead of the player's, without touching their clipboard (the
/// gate's own folder).
#[derive(Resource, Clone)]
pub struct Elsewhere(pub PathBuf);

/// Asks for the frame when the key is pressed (not while typing in the chat).
#[allow(clippy::too_many_arguments)]
fn take(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<Settings>,
    ui: Res<UiState>,
    mut shots: ResMut<Shots>,
    offscreen: Option<Res<Offscreen>>,
    elsewhere: Option<Res<Elsewhere>>,
) {
    if ui.console_open || !keys.just_pressed(settings.screenshot_key.0) {
        return;
    }
    shots.taken += 1;
    let sender = shots.sender.clone();
    let (folder, copy) = match elsewhere {
        Some(elsewhere) => (Some(elsewhere.0.clone()), false),
        None => (folder(), true),
    };
    let shot = offscreen.map_or_else(Screenshot::primary_window, |target| Screenshot::image(target.0.clone()));
    commands.spawn(shot).observe(move |captured: On<ScreenshotCaptured>| {
        let (image, sender, folder) = (captured.image.clone(), sender.clone(), folder.clone());
        AsyncComputeTaskPool::get()
            .spawn(async move {
                let _ = sender.send(keep(image, folder, copy));
            })
            .detach();
    });
}

/// Saves one frame in `folder`, and copies it to the clipboard if `copy`; runs on a worker.
fn keep(image: Image, folder: Option<PathBuf>, copy: bool) -> Kept {
    let rgba = match image.try_into_dynamic() {
        Ok(image) => image.to_rgba8(),
        Err(error) => return Kept::Failed(format!("{error}")),
    };
    let Some(folder) = folder else { return Kept::Failed("no folder for screenshots".into()) };
    if let Err(error) = std::fs::create_dir_all(&folder) {
        return Kept::Failed(format!("{}: {error}", folder.display()));
    }
    let stamp = chrono_free_stamp();
    let path = folder.join(format!("voxmar-{stamp}.png"));
    // The alpha channel holds brightness when HDR is on; the shot is opaque, as seen.
    let opaque = image::DynamicImage::ImageRgba8(rgba.clone()).to_rgb8();
    if let Err(error) = opaque.save_with_format(&path, image::ImageFormat::Png) {
        return Kept::Failed(format!("{}: {error}", path.display()));
    }
    let mut pixels = rgba.into_raw();
    pixels.chunks_exact_mut(4).for_each(|pixel| pixel[3] = 255);
    let (width, height) = (opaque.width() as usize, opaque.height() as usize);
    if !copy {
        return Kept::Saved { path, copied: Ok(false) };
    }
    let copied = arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_image(arboard::ImageData { width, height, bytes: pixels.into() }))
        .map(|()| true)
        .map_err(|error| error.to_string());
    Kept::Saved { path, copied }
}

/// The time now as a name for a file, `20261006-052501-123` (UTC), without a date
/// library.
fn chrono_free_stamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let (secs, millis) = (now.as_secs() as i64, now.subsec_millis());
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}-{millis:03}", rest / 3600, rest / 60 % 60, rest % 60)
}

/// Year, month and day of a count of days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// Says in the chat how each screenshot went.
fn report(time: Res<Time>, shots: Res<Shots>, locale: Res<Locale>, mut console: ResMut<Console>, mut last: ResMut<LastShot>) {
    let Ok(receiver) = shots.receiver.lock() else { return };
    for kept in receiver.try_iter() {
        let (kind, words) = match &kept {
            Kept::Saved { path, copied: Ok(true) } => (ChatKind::Info, locale.format("screenshot.copied", &[("file", &file_name(path))])),
            Kept::Saved { path, copied: Ok(false) } => (ChatKind::Info, locale.format("screenshot.saved", &[("file", &file_name(path))])),
            Kept::Saved { path, copied: Err(why) } => {
                warn!("screenshot: not copied to the clipboard: {why}");
                (ChatKind::Info, locale.format("screenshot.saved", &[("file", &file_name(path))]))
            }
            Kept::Failed(why) => {
                warn!("screenshot: {why}");
                (ChatKind::Error, locale.get("screenshot.failed").to_owned())
            }
        };
        console.push(kind, &words, time.elapsed_secs());
        last.0 = Some(kept);
    }
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned())
}

/// The last screenshot kept, for the gate.
#[derive(Resource, Default)]
pub struct LastShot(pub Option<Kept>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_become_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_367), (2025, 10, 6));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
