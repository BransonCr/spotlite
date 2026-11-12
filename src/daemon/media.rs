//! The system's media controls for the built-in player: media keys, and the song in the
//! system's media panel (MPRIS on Linux, so playerctl and waybar see "spotlite"; the media
//! flyout on Windows). Its own thread; the player tells it of each change.

use super::Shared;
use librespot_connect::Spirc;
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

/// A change to show in the system's media controls.
pub enum Update {
    Song { title: String, artists: String, album: String, cover: String, duration_ms: u32 },
    Playing(bool),
}

/// Starts the media controls. None if the system has none (no D-Bus session, say).
pub fn start(shared: Arc<Shared>) -> Option<Sender<Update>> {
    let (tx, rx) = channel();
    let (ready_tx, ready_rx) = channel();
    std::thread::Builder::new()
        .name("spotlite-media".into())
        .spawn(move || {
            // Windows ties the media controls to a window: a hidden one, owned by this thread.
            let hwnd = crate::platform::hidden_window();
            let config = PlatformConfig { display_name: "spotlite", dbus_name: "spotlite", hwnd };
            let mut controls = match MediaControls::new(config) {
                Ok(c) => c,
                Err(e) => {
                    super::log(&format!("no media controls: {e:?}"));
                    let _ = ready_tx.send(false);
                    return;
                }
            };
            let target = shared.clone();
            if controls.attach(move |event| on_event(&target, event)).is_err() {
                let _ = ready_tx.send(false);
                return;
            }
            let _ = ready_tx.send(true);
            run(&mut controls, &rx);
        })
        .ok()?;
    ready_rx.recv().unwrap_or(false).then_some(tx)
}

fn run(controls: &mut MediaControls, rx: &Receiver<Update>) {
    loop {
        crate::platform::pump_messages();
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Update::Song { title, artists, album, cover, duration_ms }) => {
                let _ = controls.set_metadata(MediaMetadata {
                    title: Some(&title),
                    artist: Some(&artists),
                    album: Some(&album),
                    cover_url: (!cover.is_empty()).then_some(cover.as_str()),
                    duration: Some(Duration::from_millis(u64::from(duration_ms))),
                });
            }
            Ok(Update::Playing(playing)) => {
                let _ = controls.set_playback(if playing {
                    MediaPlayback::Playing { progress: None }
                } else {
                    MediaPlayback::Paused { progress: None }
                });
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// A media key or a button in the system's media panel.
fn on_event(shared: &Shared, event: MediaControlEvent) {
    let Some(spirc): Option<Arc<Spirc>> = shared.spirc.lock().ok().and_then(|s| s.clone()) else { return };
    let _ = match event {
        MediaControlEvent::Play => spirc.play(),
        MediaControlEvent::Pause | MediaControlEvent::Stop => spirc.pause(),
        MediaControlEvent::Toggle => spirc.play_pause(),
        MediaControlEvent::Next => spirc.next(),
        MediaControlEvent::Previous => spirc.prev(),
        MediaControlEvent::SetPosition(MediaPosition(at)) => spirc.set_position_ms(at.as_millis().min(u128::from(u32::MAX)) as u32),
        MediaControlEvent::SetVolume(v) => spirc.set_volume(super::volume((v.clamp(0.0, 1.0) * 100.0).round() as u8)),
        _ => return,
    };
}
