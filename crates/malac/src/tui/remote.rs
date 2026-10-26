//! The Remote access dialog: which daemon this terminal controls, whether
//! other devices may connect and where, the paired devices, keep-awake,
//! how to connect, and a pairing page with a QR code for a phone.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::{Action, HitMap, Outcome, Target};
use super::dialog::{draw_buttons, frame_box, put};
use super::text;
use super::theme::theme;
use crate::protocol::{AwakeStatus, DeviceView, Pairing, RemoteStatus, TunnelState};
use crate::qr::{BLACK, QrCode, WHITE};
use crate::remotes::{ago, connect_steps};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Pair,
}

pub struct RemoteDialog {
    pub page: Page,
    /// `None` while loading.
    pub devices: Option<Result<Vec<DeviceView>, String>>,
    pub pairing: Option<Result<Pairing, String>>,
    pub selected: usize,
}

/// What the dialog shows about the connection.
pub struct Shown<'a> {
    /// "this computer" or the saved connection's name.
    pub connection: String,
    pub url: String,
    pub version: &'a str,
    pub remote: &'a RemoteStatus,
    pub awake: &'a AwakeStatus,
}

impl RemoteDialog {
    pub fn new() -> Self {
        Self {
            page: Page::Overview,
            devices: None,
            pairing: None,
            selected: 0,
        }
    }

    fn device_ids(&self) -> Vec<String> {
        match &self.devices {
            Some(Ok(devices)) => devices.iter().map(|d| d.id.clone()).collect(),
            _ => Vec::new(),
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        match (self.page, key.code) {
            (Page::Pair, KeyCode::Esc | KeyCode::Backspace | KeyCode::Left) => {
                self.page = Page::Overview;
                Outcome::Stay
            }
            (_, KeyCode::Esc | KeyCode::Char('q')) => Outcome::Close,
            (_, KeyCode::Char('p')) => {
                self.page = Page::Pair;
                self.pairing = None;
                Outcome::Run(Action::PairDevice)
            }
            (Page::Overview, KeyCode::Up) => {
                self.selected = self.selected.saturating_sub(1);
                Outcome::Stay
            }
            (Page::Overview, KeyCode::Down) => {
                let count = self.device_ids().len();
                self.selected = (self.selected + 1).min(count.saturating_sub(1));
                Outcome::Stay
            }
            (Page::Overview, KeyCode::Char('x') | KeyCode::Delete) => {
                match self.device_ids().get(self.selected) {
                    Some(id) => Outcome::Run(Action::RevokeDevice(id.clone())),
                    None => Outcome::Stay,
                }
            }
            (_, KeyCode::Char('r')) => Outcome::Run(Action::RemoteDialog),
            _ => Outcome::Stay,
        }
    }

    pub fn draw(&self, frame: &mut Frame, shown: &Shown, hits: &mut HitMap) {
        match self.page {
            Page::Overview => self.draw_overview(frame, shown, hits),
            Page::Pair => self.draw_pairing(frame, hits),
        }
    }

    fn draw_overview(&self, frame: &mut Frame, shown: &Shown, hits: &mut HitMap) {
        let t = theme();
        let remote = shown.remote;
        let devices = match &self.devices {
            Some(Ok(devices)) => devices.len(),
            _ => remote.devices,
        };
        let height = 18 + remote.urls.len() as u16 + devices.clamp(1, 6) as u16;
        let inner = frame_box(
            frame,
            90,
            height,
            "Remote access",
            "↑↓ device · x revoke · p pair · r refresh · esc close",
            hits,
        );
        let label = |text: &str| Span::styled(format!("{text:<15}"), t.dim());
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![
            label("Connected to"),
            Span::styled(shown.connection.clone(), t.bold()),
            Span::styled(
                format!(" · {} · malac {}", shown.url, shown.version),
                t.dim(),
            ),
        ]));
        lines.push(Line::from(vec![
            label("Remote access"),
            match remote.enabled {
                true => Span::styled("allowed", t.fg(t.accent)),
                false => Span::styled("off", t.fg(t.warning)),
            },
            Span::styled(
                match remote.enabled {
                    true => format!(
                        " · listening on {} · paired devices need their token",
                        remote.listen
                    ),
                    false => " · this machine and SSH tunnels only; start the daemon with --remote"
                        .to_owned(),
                },
                t.dim(),
            ),
        ]));
        for (index, url) in remote.urls.iter().enumerate() {
            lines.push(Line::from(vec![
                label(if index == 0 { "Addresses" } else { "" }),
                Span::styled(url.url.clone(), t.fg(t.accent)),
                Span::styled(format!("  {}", url.kind.label()), t.dim()),
            ]));
        }
        if let Some(tunnel) = &remote.tunnel {
            let (state, style) = match tunnel.state {
                TunnelState::Starting => ("starting", t.fg(t.warning)),
                TunnelState::Running => ("running", t.fg(t.accent)),
                TunnelState::Failed => ("failed", t.fg(t.error)),
                TunnelState::Stopped => ("stopped", t.dim()),
            };
            let detail = tunnel
                .error
                .as_deref()
                .map(|e| format!(" · {}", text::first_line(e)))
                .unwrap_or_default();
            lines.push(Line::from(vec![
                label("Tunnel"),
                Span::raw(format!("{} ", tunnel.provider.as_str())),
                Span::styled(state, style),
                Span::styled(detail, t.dim()),
            ]));
        }
        let awake = shown.awake;
        lines.push(Line::from(vec![
            label("Keep awake"),
            match awake.awake {
                true => Span::styled("awake", t.fg(t.accent)),
                false => Span::styled("not holding", t.dim()),
            },
            Span::styled(
                format!(
                    " · {} · mode {}{}",
                    awake.error.as_deref().unwrap_or(&awake.reason),
                    awake.mode.as_str(),
                    awake
                        .method
                        .as_deref()
                        .map(|m| format!(" ({m})"))
                        .unwrap_or_default()
                ),
                t.dim(),
            ),
        ]));
        lines.push(Line::default());
        lines.push(Line::styled(format!("Devices ({devices})"), t.bold()));
        let device_row = lines.len();
        match &self.devices {
            None => lines.push(Line::styled("  loading…", t.dim())),
            Some(Err(err)) => lines.push(Line::styled(format!("  {err}"), t.fg(t.error))),
            Some(Ok(list)) if list.is_empty() => lines.push(Line::styled(
                "  none yet: p shows a code to pair a phone, browser or computer",
                t.dim(),
            )),
            Some(Ok(list)) => {
                let first = self.selected.saturating_sub(5);
                for (index, device) in list.iter().enumerate().skip(first).take(6) {
                    let style = match index == self.selected {
                        true => t.selected(),
                        false => Style::new(),
                    };
                    let seen = device.last_seen.map(ago).unwrap_or_else(|| "never".into());
                    let from = device
                        .address
                        .as_deref()
                        .map(|a| format!(" from {a}"))
                        .unwrap_or_default();
                    let this = if device.current {
                        " · this device"
                    } else {
                        ""
                    };
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {:<22} ", text::truncate(&device.name, 22)),
                            style,
                        ),
                        Span::styled(format!("{:<8}", device.kind), style),
                        Span::styled(format!(" seen {seen}{from}{this}"), t.dim()),
                    ]));
                }
            }
        }
        lines.push(Line::default());
        lines.push(Line::styled("How to connect", t.bold()));
        let port = remote
            .listen
            .rsplit(':')
            .next()
            .unwrap_or("7700")
            .to_owned();
        for (way, how) in connect_steps(remote, &port) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {way:<14}"), t.dim()),
                Span::raw(how),
            ]));
        }
        let rows = inner.height.saturating_sub(2) as usize;
        for (row, line) in lines.into_iter().take(rows).enumerate() {
            let at = row as u16;
            if row > device_row && row <= device_row + 6 {
                let index = self.selected.saturating_sub(5) + row - device_row;
                let area = ratatui::layout::Rect::new(inner.x, inner.y + at, inner.width, 1);
                hits.push(area, Target::Pick(index));
            }
            put(frame, inner, at, line);
        }
        let mut buttons = vec![(" Pair a device  p ", Action::PairDevice)];
        if let Some(id) = self.device_ids().get(self.selected) {
            buttons.push((" Revoke  x ", Action::RevokeDevice(id.clone())));
        }
        buttons.push((" Close  esc ", Action::CloseDialog));
        draw_buttons(
            frame,
            inner,
            inner.height.saturating_sub(1),
            &buttons,
            0,
            hits,
        );
    }

    fn draw_pairing(&self, frame: &mut Frame, hits: &mut HitMap) {
        let t = theme();
        let pairing = match &self.pairing {
            Some(Ok(pairing)) => pairing,
            other => {
                let inner = frame_box(frame, 60, 5, "Pair a device", "esc back", hits);
                let line = match other {
                    Some(Err(err)) => Line::styled(err.clone(), t.fg(t.error)),
                    _ => Line::styled("asking the daemon for a code…", t.dim()),
                };
                put(frame, inner, 0, line);
                return;
            }
        };
        let Some(best) = pairing.links.first() else {
            let inner = frame_box(frame, 60, 5, "Pair a device", "esc back", hits);
            put(frame, inner, 0, Line::raw("the daemon reported no address"));
            return;
        };
        let qr = QrCode::encode(&best.url);
        let qr_rows = qr.as_ref().map(|q| q.half_rows(2)).unwrap_or_default();
        let width = qr_rows.first().map_or(0, Vec::len).max(60) as u16 + 8;
        let height = qr_rows.len() as u16 + 9 + pairing.links.len() as u16;
        let inner = frame_box(
            frame,
            width.max(84),
            height,
            "Pair a device",
            "p new code · esc back",
            hits,
        );
        let minutes = (pairing.expires - chrono::Utc::now()).num_minutes() + 1;
        put(
            frame,
            inner,
            0,
            Line::from(vec![
                Span::styled("Code  ", t.dim()),
                Span::styled(
                    pairing.code.clone(),
                    t.bold().add_modifier(Modifier::REVERSED),
                ),
                Span::styled(format!("  valid {minutes} min, for one device"), t.dim()),
            ]),
        );
        let mut row = 2u16;
        let fits = inner.height as usize >= qr_rows.len() + 4 + pairing.links.len();
        if fits {
            let pixel = |dark: bool| match dark {
                true => Color::Indexed(BLACK),
                false => Color::Indexed(WHITE),
            };
            for cells in &qr_rows {
                let spans: Vec<Span<'static>> = cells
                    .iter()
                    .map(|&(upper, lower)| {
                        Span::styled("▀", Style::new().fg(pixel(upper)).bg(pixel(lower)))
                    })
                    .collect();
                put(frame, inner, row, Line::from(spans));
                row += 1;
            }
        } else {
            put(
                frame,
                inner,
                row,
                Line::styled("make the terminal taller to see the QR code", t.dim()),
            );
            row += 1;
        }
        row += 1;
        put(
            frame,
            inner,
            row,
            Line::from(vec![
                Span::styled("Scan it, or open  ", t.dim()),
                Span::styled(
                    best.url.clone(),
                    t.fg(t.accent).add_modifier(Modifier::UNDERLINED),
                ),
            ]),
        );
        row += 1;
        for link in &pairing.links {
            let way = link.kind.short();
            put(
                frame,
                inner,
                row,
                Line::from(vec![
                    Span::styled(format!("{way:<14}"), t.dim()),
                    Span::raw(format!(
                        "malac remote add <name> {} --pair {}",
                        link.base, pairing.code
                    )),
                ]),
            );
            row += 1;
        }
    }
}
