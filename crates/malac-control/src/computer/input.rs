//! Mouse and keyboard input through enigo. Its handles are not `Send` on every
//! platform, so one dedicated thread owns them and runs queued jobs.

use std::sync::mpsc;

use anyhow::{Context, Result, anyhow, bail};
use enigo::{Direction, Enigo, InputError, Keyboard, NewConError, Settings};
use tokio::sync::{Mutex, oneshot};

use crate::keys::{Key, KeyCombo};

type Job = Box<dyn FnOnce(&mut Enigo) + Send>;

#[derive(Default)]
pub struct Input {
    worker: Mutex<Option<mpsc::Sender<Job>>>,
}

impl Input {
    /// Runs `action` on the input thread, starting it on first use.
    pub async fn run<T, F>(&self, action: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Enigo) -> Result<T, InputError> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |enigo| {
            let _ = tx.send(action(enigo));
        });
        let mut worker = self.worker.lock().await;
        let sender = match worker.as_ref() {
            Some(sender) => sender.clone(),
            None => worker.insert(spawn().await?).clone(),
        };
        if sender.send(job).is_err() {
            worker.take();
            bail!("the input thread stopped; try again");
        }
        drop(worker);
        rx.await
            .context("the input thread stopped")?
            .map_err(|err| anyhow!("input failed: {err}"))
    }
}

async fn spawn() -> Result<mpsc::Sender<Job>> {
    let (jobs, queue) = mpsc::channel::<Job>();
    let (ready_tx, ready) = oneshot::channel();
    std::thread::Builder::new()
        .name("malac-input".into())
        .spawn(move || {
            let settings = Settings {
                open_prompt_to_get_permissions: false,
                ..Settings::default()
            };
            let mut enigo = match Enigo::new(&settings) {
                Ok(enigo) => enigo,
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            for job in queue {
                job(&mut enigo);
            }
        })
        .context("failed to start the input thread")?;
    match ready.await {
        Ok(Ok(())) => Ok(jobs),
        Ok(Err(err)) => Err(connection_error(err)),
        Err(_) => bail!("the input thread exited during startup"),
    }
}

fn connection_error(err: NewConError) -> anyhow::Error {
    match err {
        #[cfg(target_os = "macos")]
        NewConError::NoPermission => super::permissions::accessibility_error(),
        other => anyhow!("cannot simulate input: {other}"),
    }
}

/// Holds the combo's modifiers, taps the key, and releases the modifiers even
/// when the tap fails.
pub fn press(enigo: &mut Enigo, combo: KeyCombo, key: enigo::Key) -> Result<(), InputError> {
    let m = combo.modifiers;
    let modifiers: Vec<enigo::Key> = [
        (m.ctrl, enigo::Key::Control),
        (m.alt, enigo::Key::Alt),
        (m.shift, enigo::Key::Shift),
        (m.meta, enigo::Key::Meta),
    ]
    .into_iter()
    .filter_map(|(held, key)| held.then_some(key))
    .collect();
    for (pressed, modifier) in modifiers.iter().enumerate() {
        if let Err(err) = enigo.key(*modifier, Direction::Press) {
            release(enigo, &modifiers[..pressed]);
            return Err(err);
        }
    }
    let result = enigo.key(key, Direction::Click);
    release(enigo, &modifiers);
    result
}

fn release(enigo: &mut Enigo, modifiers: &[enigo::Key]) {
    for modifier in modifiers.iter().rev() {
        let _ = enigo.key(*modifier, Direction::Release);
    }
}

pub fn enigo_key(key: Key) -> Result<enigo::Key> {
    use enigo::Key as E;
    const FUNCTION_KEYS: [E; 20] = [
        E::F1,
        E::F2,
        E::F3,
        E::F4,
        E::F5,
        E::F6,
        E::F7,
        E::F8,
        E::F9,
        E::F10,
        E::F11,
        E::F12,
        E::F13,
        E::F14,
        E::F15,
        E::F16,
        E::F17,
        E::F18,
        E::F19,
        E::F20,
    ];
    Ok(match key {
        Key::Char(c) => E::Unicode(c),
        Key::Enter => E::Return,
        Key::Tab => E::Tab,
        Key::Space => E::Space,
        Key::Backspace => E::Backspace,
        Key::Delete => E::Delete,
        Key::Escape => E::Escape,
        Key::Up => E::UpArrow,
        Key::Down => E::DownArrow,
        Key::Left => E::LeftArrow,
        Key::Right => E::RightArrow,
        Key::Home => E::Home,
        Key::End => E::End,
        Key::PageUp => E::PageUp,
        Key::PageDown => E::PageDown,
        Key::CapsLock => E::CapsLock,
        #[cfg(not(target_os = "macos"))]
        Key::Insert => E::Insert,
        #[cfg(target_os = "macos")]
        Key::Insert => bail!("macOS has no Insert key"),
        Key::F(n) => *usize::from(n)
            .checked_sub(1)
            .and_then(|i| FUNCTION_KEYS.get(i))
            .ok_or_else(|| anyhow!("unsupported function key F{n}"))?,
    })
}
